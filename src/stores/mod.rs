use std::collections::BTreeSet;
use std::mem::swap;
use std::{fmt::Debug, sync::Arc};

use async_trait::async_trait;
use failure::Error;
use futures::FutureExt;

use crate::protocol::{Event, RoomState, RoomVersion};

pub mod backed;
pub mod memory;
pub mod postgres;

// Ideally this trait would have lots of generic parameters so that we don't
// need to do lots of clones and stuff. Alas, we need this to be object safe
// so that it can be used in trait factories and such.
#[async_trait]
pub trait EventStore<R: RoomVersion>: Send + Sync + 'static {
    async fn insert_events(&self, events: Vec<R::Event>) -> Result<(), Error>;

    async fn insert_event(&self, event: R::Event) -> Result<(), Error> {
        self.insert_events(vec![event]).await
    }

    /// Return list of events we have not persisted.
    async fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> Result<Vec<String>, Error>;

    async fn get_events(
        &self,
        event_ids: &[&str],
    ) -> Result<Vec<R::Event>, Error>;

    async fn get_event(
        &self,
        event_id: &str,
    ) -> Result<Option<R::Event>, Error> {
        self.get_events(&[event_id])
            .map(|r| r.map(|v| v.into_iter().next()))
            .await
    }

    async fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<String>>,
    ) -> Result<Vec<R::Event>, Error> {
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
                        return Err(format_err!("Don't have full auth chain"));
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
        let intersection = auth_chains.iter().fold(union.clone(), |u, x| {
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

    async fn get_backfill(
        &self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> Result<Vec<R::Event>, Error> {
        let mut queue = event_ids;
        let mut to_return = Vec::new();

        'top: while !queue.is_empty() {
            let m: Vec<_> = queue.drain(..).collect();
            let events = self
                .get_events(&m.iter().map(|e| e as &str).collect::<Vec<_>>())
                .await?;
            for event in events {
                queue.extend(
                    event.prev_event_ids().into_iter().map(ToString::to_string),
                );
                to_return.push(event);

                if to_return.len() >= limit {
                    break 'top;
                }
            }
        }

        Ok(to_return)
    }

    /// Returns the event IDs of all events in the given events' auth chain.
    ///
    /// The auth chain of an event is the graph of events with auth events as
    /// the edges, i.e. the event's auth events, and their auth events,
    /// recursively.
    async fn get_auth_chain_ids(
        &self,
        event_ids: Vec<String>,
    ) -> Result<Vec<R::Event>, Error> {
        let mut to_return = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue = event_ids;

        while !queue.is_empty() {
            let m: Vec<_> = queue.drain(..).collect();
            let events = self
                .get_events(&m.iter().map(|e| e as &str).collect::<Vec<_>>())
                .await?;
            for event in events {
                queue.extend(
                    event
                        .auth_event_ids()
                        .into_iter()
                        .map(ToString::to_string)
                        .filter(|e_id| !seen.contains(e_id)),
                );
                if !seen.contains(event.event_id()) {
                    seen.insert(event.event_id().to_string());
                    to_return.push(event);
                }
            }
        }

        Ok(to_return)
    }
}

#[async_trait]
impl<R, E> EventStore<R> for Arc<E>
where
    E: EventStore<R> + ?Sized,
    R: RoomVersion,
{
    async fn insert_events(&self, events: Vec<R::Event>) -> Result<(), Error> {
        self.as_ref().insert_events(events).await
    }

    async fn insert_event(&self, event: R::Event) -> Result<(), Error> {
        self.as_ref().insert_event(event).await
    }

    /// Return list of events we have not persisted.
    async fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> Result<Vec<String>, Error> {
        self.as_ref().missing_events(event_ids).await
    }

    async fn get_events(
        &self,
        event_ids: &[&str],
    ) -> Result<Vec<R::Event>, Error> {
        self.as_ref().get_events(event_ids).await
    }

    async fn get_event(
        &self,
        event_id: &str,
    ) -> Result<Option<R::Event>, Error> {
        self.as_ref().get_event(event_id).await
    }

    async fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<String>>,
    ) -> Result<Vec<R::Event>, Error> {
        self.as_ref().get_conflicted_auth_chain(event_ids).await
    }

    async fn get_backfill(
        &self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> Result<Vec<R::Event>, Error> {
        self.as_ref().get_backfill(event_ids, limit).await
    }
}

#[async_trait]
pub trait StateStore<R: RoomVersion, S: RoomState<String>>:
    Send + Sync + 'static
{
    async fn insert_state<'a>(
        &'a self,
        event: &R::Event,
        state: &'a mut S,
    ) -> Result<(), Error>;

    async fn get_state_before(
        &self,
        event_id: &str,
    ) -> Result<Option<S>, Error>;

    async fn get_state_after(
        &self,
        event_ids: &[&str],
    ) -> Result<Option<S>, Error>;
}

#[async_trait]
pub trait RoomStore<E: Event + 'static>: Send + Sync {
    /// Insert non-rejected events that should be used for calculating forward
    /// extremities.
    async fn insert_new_events(&self, events: Vec<E>) -> Result<(), Error>;

    async fn insert_new_event(&self, event: E) -> Result<(), Error> {
        self.insert_new_events(vec![event]).await
    }

    /// Get the forward extremities for a room.
    async fn get_forward_extremities(
        &self,
        room_id: String,
    ) -> Result<BTreeSet<String>, Error>;
}

pub trait StoreFactory<S: RoomState<String>>: Debug {
    fn get_event_store<R: RoomVersion>(&self) -> Arc<dyn EventStore<R>>;
    fn get_state_store<R: RoomVersion>(&self) -> Arc<dyn StateStore<R, S>>;
    fn get_room_store<R: RoomVersion>(&self) -> Arc<dyn RoomStore<R::Event>>;

    fn get_room_version_store(&self) -> Arc<dyn RoomVersionStore>;
}

#[async_trait]
pub trait RoomVersionStore: Send + Sync {
    async fn get_room_version(
        &self,
        room_id: &str,
    ) -> Result<Option<&'static str>, Error>;

    async fn set_room_version<'a>(
        &'a self,
        room_id: &'a str,
        version: &'static str,
    ) -> Result<(), Error>;
}
