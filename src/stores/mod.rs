use std::collections::BTreeSet;
use std::mem::swap;
use std::sync::Arc;

use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;

use crate::protocol::{Event, RoomState, RoomVersion};

pub mod backed;
pub mod memory;
pub mod postgres;

// Ideally this trait would have lots of generic parameters so that we don't
// need to do lots of clones and stuff. Alas, we need this to be object safe
// so that it can be used in trait factories and such.

pub trait EventStore<R: RoomVersion>: Send + Sync + 'static {
    fn insert_events(
        &self,
        events: Vec<R::Event>,
    ) -> BoxFuture<Result<(), Error>>;

    fn insert_event(&self, event: R::Event) -> BoxFuture<Result<(), Error>> {
        self.insert_events(vec![event])
    }

    /// Return list of events we have not persisted.
    fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<String>, Error>>;

    fn get_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<R::Event>, Error>>;

    fn get_event(
        &self,
        event_id: &str,
    ) -> BoxFuture<Result<Option<R::Event>, Error>> {
        self.get_events(&[event_id])
            .map(|r| r.map(|v| v.into_iter().next()))
            .boxed()
    }

    fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<String>>,
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        async move {
            let mut auth_chains: Vec<BTreeSet<String>> =
                Vec::with_capacity(event_ids.len());

            for group in &event_ids {
                let mut group_chain = BTreeSet::new();

                let mut stack: Vec<String> = group.clone();
                let mut new_stack = Vec::new();

                while !stack.is_empty() {
                    for event_id in &stack {
                        if group_chain.contains(event_id) {
                            continue;
                        }

                        if let Some(event) = self.get_event(&event_id).await? {
                            new_stack.extend(
                                event
                                    .auth_event_ids()
                                    .into_iter()
                                    .map(|a| a.to_string()),
                            );
                            group_chain.insert(event.event_id().to_string());
                        } else {
                            return Err(format_err!(
                                "Don't have full auth chain"
                            ));
                        }
                    }
                    swap(&mut stack, &mut new_stack);
                    new_stack.clear();
                }

                auth_chains.push(group_chain);
            }

            let union = auth_chains.iter().fold(BTreeSet::new(), |u, x| {
                x.union(&u).map(ToString::to_string).collect()
            });
            let intersection =
                auth_chains.iter().fold(union.clone(), |u, x| {
                    x.intersection(&u).map(ToString::to_string).collect()
                });

            let differences = union.difference(&intersection);

            let mut events = Vec::new();
            for e in differences {
                if let Some(event) = self.get_event(e).await? {
                    events.push(event);
                }
            }

            Ok(events)
        }
        .boxed()
    }

    fn get_backfill(
        &self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        async move {
            let mut queue = event_ids;
            let mut to_return = Vec::new();

            'top: while !queue.is_empty() {
                let m: Vec<_> = queue.drain(..).collect();
                let events = self
                    .get_events(
                        &m.iter().map(|e| e as &str).collect::<Vec<_>>(),
                    )
                    .await?;
                for event in events {
                    queue.extend(
                        event
                            .prev_event_ids()
                            .into_iter()
                            .map(ToString::to_string),
                    );
                    to_return.push(event);

                    if to_return.len() >= limit {
                        break 'top;
                    }
                }
            }

            Ok(to_return)
        }
        .boxed()
    }
}

impl<R, E> EventStore<R> for Arc<E>
where
    E: EventStore<R> + ?Sized,
    R: RoomVersion,
{
    fn insert_events(
        &self,
        events: Vec<R::Event>,
    ) -> BoxFuture<Result<(), Error>> {
        self.as_ref().insert_events(events)
    }

    fn insert_event(&self, event: R::Event) -> BoxFuture<Result<(), Error>> {
        self.as_ref().insert_event(event)
    }

    /// Return list of events we have not persisted.
    fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<String>, Error>> {
        self.as_ref().missing_events(event_ids)
    }

    fn get_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        self.as_ref().get_events(event_ids)
    }

    fn get_event(
        &self,
        event_id: &str,
    ) -> BoxFuture<Result<Option<R::Event>, Error>> {
        self.as_ref().get_event(event_id)
    }

    fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<String>>,
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        self.as_ref().get_conflicted_auth_chain(event_ids)
    }

    fn get_backfill(
        &self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        self.as_ref().get_backfill(event_ids, limit)
    }
}

pub trait StateStore<R: RoomVersion, S: RoomState<String>>:
    Send + Sync + 'static
{
    fn insert_state(
        &self,
        event: &R::Event,
        state: S,
    ) -> BoxFuture<Result<(), Error>>;

    fn get_state_before(
        &self,
        event_id: &str,
    ) -> BoxFuture<Result<Option<S>, Error>>;

    fn get_state_after(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Option<S>, Error>>;
}

pub trait RoomStore<E: Event>: Send + Sync {
    /// Insert non-rejected events that should be used for calculating forward
    /// extremities.
    fn insert_new_events(&self, events: Vec<E>)
        -> BoxFuture<Result<(), Error>>;

    fn insert_new_event(&self, event: E) -> BoxFuture<Result<(), Error>> {
        self.insert_new_events(vec![event])
    }

    /// Get the forward extremities for a room.
    fn get_forward_extremities(
        &self,
        room_id: String,
    ) -> BoxFuture<Result<BTreeSet<String>, Error>>;
}

// impl<T, R: RoomVersion, S: RoomState> EventStore<R, S> for &'static T where
//     T: EventStore<R, S>
// {
// }

// impl<T, R: RoomVersion, S: RoomState> EventStore<R, S> for Box<T> where
//     T: EventStore<R, S>
// {
// }

pub trait StoreFactory<S: RoomState<String>> {
    fn get_event_store<R: RoomVersion>(&self) -> Arc<dyn EventStore<R>>;
    fn get_state_store<R: RoomVersion>(&self) -> Arc<dyn StateStore<R, S>>;
    fn get_room_store<R: RoomVersion>(&self) -> Arc<dyn RoomStore<R::Event>>;

    fn get_room_version_store(&self) -> Arc<dyn RoomVersionStore>;
}

pub trait RoomVersionStore: Send + Sync {
    fn get_room_version(
        &self,
        room_id: &str,
    ) -> BoxFuture<Result<Option<&'static str>, Error>>;

    fn set_room_version(
        &self,
        room_id: &str,
        version: &'static str,
    ) -> BoxFuture<Result<(), Error>>;
}
