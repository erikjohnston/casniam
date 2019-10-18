use crate::protocol::{Event, RoomState, RoomVersion};

use std::collections::BTreeSet;
use std::future::Future;
use std::iter;
use std::mem::swap;
use std::pin::Pin;

use failure::Error;
use futures::FutureExt;

pub mod backed;
pub mod memory;

pub trait EventStore: Clone + 'static {
    type Event: Event + Send;
    type RoomState: RoomState;
    type RoomVersion: RoomVersion<Event = Self::Event>;

    fn insert_events(
        &self,
        events: impl IntoIterator<Item = (Self::Event, Self::RoomState)>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>>;

    fn insert_event(
        &self,
        event: Self::Event,
        state: Self::RoomState,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
        self.insert_events(iter::once((event, state)))
    }

    fn missing_events<I: IntoIterator<Item = impl AsRef<str> + ToString>>(
        &self,
        event_ids: I,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>>>>;

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>>;

    fn get_event(
        &self,
        event_id: impl AsRef<str>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Self::Event>, Error>>>> {
        self.get_events(iter::once(event_id))
            .map(|r| r.map(|v| v.into_iter().next()))
            .boxed_local()
    }

    fn get_state_for<T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<dyn Future<Output = Result<Option<Self::RoomState>, Error>>>>;

    fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<impl AsRef<str>>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>> {
        let store = self.clone();

        let event_ids: Vec<Vec<String>> = event_ids
            .into_iter()
            .map(|v| v.into_iter().map(|e| e.as_ref().to_string()).collect())
            .collect();

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

                        if let Some(event) = store.get_event(&event_id).await? {
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
                if let Some(event) = store.get_event(e).await? {
                    events.push(event);
                }
            }

            Ok(events)
        }
        .boxed_local()
    }

    fn get_backfill(
        &self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>> {
        let database = self.clone();

        async move {
            let mut queue = event_ids;
            let mut to_return = Vec::new();

            'top: while !queue.is_empty() {
                let events = database.get_events(queue.drain(..)).await?;
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
        .boxed_local()
    }
}

pub trait RoomStore: Clone + 'static {
    type Event: Event;

    /// Insert non-rejected events that should be used for calculating forward
    /// extremities.
    fn insert_new_events(
        &self,
        events: impl IntoIterator<Item = Self::Event>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>>;

    fn insert_new_event(
        &self,
        event: Self::Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
        self.insert_new_events(iter::once(event))
    }

    /// Get the forward extremities for a room.
    fn get_forward_extremities(
        &self,
        room_id: String,
    ) -> Pin<Box<dyn Future<Output = Result<BTreeSet<String>, Error>>>>;
}
