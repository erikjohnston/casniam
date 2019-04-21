use crate::protocol::{Event, EventStore, RoomState};
use crate::state_map::StateMap;

use failure::Error;
use futures::{future, Future, FutureExt};

use std::collections::BTreeMap;
use std::pin::Pin;


pub struct MemoryEventStore<E> {
    event_map: BTreeMap<String, E>,
    state_map: BTreeMap<String, StateMap<E>>,
}

impl<E> EventStore for MemoryEventStore<E>
where
    E: Event + 'static,
{
    type Event = E;

    fn missing_events<'a, I: IntoIterator<Item = impl AsRef<str> + ToString>>(
        &self,
        event_ids: I,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
        future::ok(
            event_ids
                .into_iter()
                .filter(|e| !self.event_map.contains_key(e.as_ref()))
                .map(|e| e.to_string())
                .collect()
        )
        .boxed()
    }

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<Future<Output = Result<Vec<E>, Error>>>> {
        future::ok(
            event_ids
                .into_iter()
                .filter_map(|e| self.event_map.get(e.as_ref()))
                .cloned()
                .collect()
        )
        .boxed()
    }

    fn get_state_for<S: RoomState, T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<Future<Output = Result<Option<S>, Error>>>> {
        unimplemented!()
    }
}
