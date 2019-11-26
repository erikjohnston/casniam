use crate::protocol::{Event, RoomState, RoomStateResolver, RoomVersion};
use crate::stores::memory::{new_memory_store, MemoryEventStore};
use crate::stores::EventStore;

use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;

use std::collections::BTreeSet;
use std::iter::FromIterator;

#[derive(Clone)]
pub struct BackedStore<R, S, ES>
where
    R: RoomVersion,
    S: RoomState,
{
    store: ES,
    memory: MemoryEventStore<R, S>,
}

impl<R, S, ES> BackedStore<R, S, ES>
where
    R: RoomVersion,
    S: RoomState,
{
    pub fn new(store: ES) -> BackedStore<R, S, ES> {
        BackedStore {
            store,
            memory: new_memory_store::<R, S>(),
        }
    }
}

impl<R, S, ES> EventStore<R, S> for BackedStore<R, S, ES>
where
    R: RoomVersion,
    S: RoomState,
    ES: EventStore<R, S> + Clone,
{
    fn insert_events(
        &self,
        events: Vec<(R::Event, S)>,
    ) -> BoxFuture<Result<(), Error>> {
        self.memory.insert_events(events)
    }

    fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<String>, Error>> {
        let store = self.clone();

        let event_ids: Vec<_> =
            event_ids.iter().map(|&e| e.to_string()).collect();

        async move {
            let mut missing = store
                .memory
                .missing_events(
                    &event_ids.iter().map(|e| e as &str).collect::<Vec<_>>(),
                )
                .await?;

            if !missing.is_empty() {
                missing = store
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
        let store = self.clone();

        let event_ids: Vec<_> =
            event_ids.iter().map(|&e| e.to_string()).collect();

        async move {
            let mut events = store
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
                    &mut store
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

    fn get_state_for(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Option<S>, Error>> {
        let store = self.clone();

        let event_ids: Vec<_> =
            event_ids.iter().map(|&e| e.to_string()).collect();

        async move {
            let mut states = Vec::with_capacity(event_ids.len());

            for event_id in &event_ids {
                if let Some(s) = store.memory.get_state_for(&[event_id]).await?
                {
                    states.push(s.into_iter().collect());
                } else if let Some(s) =
                    store.store.get_state_for(&[event_id]).await?
                {
                    states.push(s);
                } else {
                    // We couldn't find one of the event IDs, so we bail.
                    return Ok(None);
                }
            }

            let state = R::State::resolve_state(states, &store).await?;
            Ok(Some(state))
        }
        .boxed()
    }
}
