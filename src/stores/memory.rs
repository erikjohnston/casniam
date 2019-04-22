use crate::protocol::{Event, EventStore, RoomState};
use crate::state_map::StateMap;

use failure::Error;
use futures::{future, Future, FutureExt};

use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct MemoryEventStoreInner<E: Clone + fmt::Debug> {
    event_map: BTreeMap<String, E>,
    state_map: BTreeMap<String, StateMap<E>>,
}

pub type MemoryEventStore<E: Clone + fmt::Debug> = Arc<Mutex<MemoryEventStoreInner<E>>>;

impl<E> EventStore for MemoryEventStore<E>
where
    E: Event + 'static,
{
    type Event = E;
    type RoomState = StateMap<String>;

    fn missing_events<
        'a,
        I: IntoIterator<Item = impl AsRef<str> + ToString>,
    >(
        &self,
        event_ids: I,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
        let store = self.lock().expect("Mutex poisoned");

        future::ok(
            event_ids
                .into_iter()
                .filter(|e| !store.event_map.contains_key(e.as_ref()))
                .map(|e| e.to_string())
                .collect(),
        )
        .boxed()
    }

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<Future<Output = Result<Vec<E>, Error>>>> {
        let store = self.lock().expect("Mutex poisoned");

        future::ok(
            event_ids
                .into_iter()
                .filter_map(|e| store.event_map.get(e.as_ref()))
                .cloned()
                .collect(),
        )
        .boxed()
    }

    fn get_state_for<T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<Future<Output = Result<Option<Self::RoomState>, Error>>>> {
        unimplemented!()
    }
}
