use crate::protocol::{Event, EventStore, RoomStateResolver, RoomVersion};
use crate::state_map::StateMap;

use failure::Error;
use futures::{future, Future, FutureExt};

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub struct MemoryEventStoreInner<R: RoomVersion> {
    event_map: BTreeMap<String, R::Event>,
    state_map: BTreeMap<String, StateMap<R::Event>>,
}

pub type MemoryEventStore<R> = Arc<RwLock<MemoryEventStoreInner<R>>>;

impl<R> EventStore for MemoryEventStore<R>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
{
    type Event = R::Event;
    type RoomState = StateMap<String>;
    type RoomVersion = R;

    fn missing_events<
        'a,
        I: IntoIterator<Item = impl AsRef<str> + ToString>,
    >(
        &self,
        event_ids: I,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
        let store = self.read().expect("Mutex poisoned");

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
    ) -> Pin<Box<Future<Output = Result<Vec<Self::Event>, Error>>>> {
        let store = self.read().expect("Mutex poisoned");

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
        let mut states: Vec<Self::RoomState> =
            Vec::with_capacity(event_ids.len());

        {
            let store = self.read().expect("Mutex poisoned");
            for e_id in event_ids {
                if let Some(state) = store.state_map.get(e_id.as_ref()) {
                    let state_ids = state
                        .iter()
                        .map(|(k, e)| (k, e.event_id().to_string()))
                        .collect();
                    states.push(state_ids)
                } else {
                    // We don't have all the state.
                    return future::ok(None).boxed();
                }
            }
        }

        let store = self.clone();

        async move {
            let state = await!(R::State::resolve_state(states, &store))?;

            Ok(Some(state))
        }
            .boxed()
    }
}
