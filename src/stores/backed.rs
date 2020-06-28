use crate::protocol::{Event, RoomState, RoomVersion};
use crate::stores::memory::{new_memory_store, MemoryEventStore};
use crate::stores::EventStore;

use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;

use std::collections::BTreeSet;
use std::iter::FromIterator;
use std::sync::Arc;

pub struct BackedStore<R, S, ES: ?Sized>
where
    R: RoomVersion,
    S: RoomState,
{
    store: Arc<ES>,
    memory: MemoryEventStore<R, S>,
}

impl<R, S, ES: ?Sized> Clone for BackedStore<R, S, ES>
where
    R: RoomVersion,
    S: RoomState,
{
    fn clone(&self) -> Self {
        BackedStore {
            store: self.store.clone(),
            memory: self.memory.clone(),
        }
    }
}

impl<R, S, ES> BackedStore<R, S, ES>
where
    R: RoomVersion,
    S: RoomState,
    ES: EventStore<R> + ?Sized,
{
    pub fn new(store: Arc<ES>) -> BackedStore<R, S, ES> {
        BackedStore {
            store,
            memory: new_memory_store::<R, S>(),
        }
    }
}

impl<R, S, ES> EventStore<R> for BackedStore<R, S, ES>
where
    R: RoomVersion,
    S: RoomState,
    ES: EventStore<R> + ?Sized,
{
    fn insert_events(
        &self,
        events: Vec<R::Event>,
    ) -> BoxFuture<Result<(), Error>> {
        self.memory.insert_events(events)
    }

    fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<String>, Error>> {
        let event_ids: Vec<_> =
            event_ids.iter().map(|&e| e.to_string()).collect();

        async move {
            let mut missing = self
                .memory
                .missing_events(
                    &event_ids.iter().map(|e| e as &str).collect::<Vec<_>>(),
                )
                .await?;

            if !missing.is_empty() {
                missing = self
                    .store
                    .missing_events(
                        &missing.iter().map(|e| e as &str).collect::<Vec<_>>(),
                    )
                    .await?;
            }

            Ok(missing)
        }
        .boxed()
    }

    fn get_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        let event_ids: Vec<_> =
            event_ids.iter().map(|&e| e.to_string()).collect();

        async move {
            let mut events = self
                .memory
                .get_events(
                    &event_ids.iter().map(|e| e as &str).collect::<Vec<_>>(),
                )
                .await?;

            let mut missing = BTreeSet::from_iter(event_ids);
            for e in &events {
                missing.remove(e.event_id());
            }

            if !missing.is_empty() {
                events.append(
                    &mut self
                        .store
                        .get_events(
                            &missing
                                .iter()
                                .map(|e| e as &str)
                                .collect::<Vec<_>>(),
                        )
                        .await?,
                );
            }

            Ok(events)
        }
        .boxed()
    }
}
