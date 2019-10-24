use crate::protocol::{Event, RoomStateResolver, RoomVersion};
use crate::stores::memory::{new_memory_store, MemoryEventStore};
use crate::stores::EventStore;

use failure::Error;
use futures::{Future, FutureExt};

use std::collections::BTreeSet;
use std::iter::FromIterator;
use std::pin::Pin;

#[derive(Clone)]
pub struct BackedStore<S: EventStore> {
    store: S,
    memory: MemoryEventStore<S::RoomVersion, S::RoomState>,
}

impl<S: EventStore> BackedStore<S> {
    pub fn new(store: S) -> BackedStore<S> {
        BackedStore {
            store,
            memory: new_memory_store::<S::RoomVersion, S::RoomState>(),
        }
    }
}

impl<S: EventStore> EventStore for BackedStore<S> {
    type Event = S::Event;
    type RoomState = S::RoomState;
    type RoomVersion = S::RoomVersion;

    fn insert_events(
        &self,
        events: impl IntoIterator<Item = (Self::Event, Self::RoomState)>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
        self.memory.insert_events(events)
    }

    fn missing_events<I: IntoIterator<Item = impl AsRef<str> + ToString>>(
        &self,
        event_ids: I,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>>>> {
        let store = self.clone();

        let event_ids: Vec<_> = event_ids
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect();

        async move {
            let mut missing = store.memory.missing_events(event_ids).await?;

            if !missing.is_empty() {
                missing = store.store.missing_events(missing).await?;
            }

            Ok(missing)
        }
        .boxed_local()
    }

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>> {
        let store = self.clone();

        let event_ids: Vec<_> = event_ids
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect();

        async move {
            let mut events = store.memory.get_events(&event_ids).await?;

            let mut missing = BTreeSet::from_iter(event_ids);
            for e in &events {
                missing.remove(e.event_id());
            }

            if !missing.is_empty() {
                events.append(&mut store.store.get_events(missing).await?);
            }

            Ok(events)
        }
        .boxed_local()
    }

    fn get_state_for<T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<dyn Future<Output = Result<Option<Self::RoomState>, Error>>>>
    {
        let store = self.clone();

        let event_ids: Vec<_> = event_ids
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect();

        async move {
            let mut states = Vec::with_capacity(event_ids.len());

            for event_id in &event_ids {
                if let Some(s) = store.memory.get_state_for(&[event_id]).await?
                {
                    states.push(s.into_iter().collect());
                }
                if let Some(s) = store.store.get_state_for(&[event_id]).await? {
                    states.push(s);
                }
                return Ok(None);
            }

            let state =
                <<S as EventStore>::RoomVersion as RoomVersion>::State::resolve_state(states, &store).await?;
            Ok(Some(state))
        }
        .boxed_local()
    }
}
