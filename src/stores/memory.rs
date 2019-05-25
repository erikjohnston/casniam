use crate::protocol::{Event, EventStore, RoomStateResolver, RoomVersion};
use crate::state_map::StateMap;

use failure::Error;
use futures::{future, Future, FutureExt};

use std::collections::{BTreeMap, BTreeSet};
use std::mem::swap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

#[derive(Default, Debug)]
pub struct MemoryEventStoreInner<R: RoomVersion> {
    event_map: BTreeMap<String, R::Event>,
    state_map: BTreeMap<String, StateMap<String>>,
}

pub type MemoryEventStore<R> = Arc<RwLock<MemoryEventStoreInner<R>>>;

pub fn new_memory_store<R: RoomVersion>() -> MemoryEventStore<R> {
    Arc::new(RwLock::new(MemoryEventStoreInner {
        event_map: BTreeMap::new(),
        state_map: BTreeMap::new(),
    }))
}

impl<R> EventStore for MemoryEventStore<R>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
{
    type Event = R::Event;
    type RoomState = StateMap<String>;
    type RoomVersion = R;

    fn insert_events(
        &self,
        events: impl IntoIterator<Item = (Self::Event, Self::RoomState)>,
    ) -> Pin<Box<Future<Output = Result<(), Error>>>> {
        let mut store = self.write().expect("Mutex poisoned");

        for (event, state) in events {
            store.state_map.insert(event.event_id().to_string(), state);
            store.event_map.insert(event.event_id().to_string(), event);
        }

        future::ok(()).boxed()
    }

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
        .boxed_local()
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
                let e_id = e_id.as_ref();

                if let (Some(state), Some(event)) =
                    (store.state_map.get(e_id), store.event_map.get(e_id))
                {
                    let mut state_ids: Self::RoomState =
                        state.iter().map(|(k, e)| (k, e.to_string())).collect();

                    // Since we're getting the resolved state we need to add the
                    // event itself if its a state event.
                    if let Some(state_key) = event.state_key() {
                        state_ids.insert(
                            event.event_type(),
                            state_key,
                            event.event_id().to_string(),
                        );
                    }

                    states.push(state_ids);
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
            .boxed_local()
    }

    fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<impl AsRef<str>>>,
    ) -> Pin<Box<Future<Output = Result<Vec<Self::Event>, Error>>>> {
        let store = self.read().expect("Mutex poisoned");

        let mut auth_chains: Vec<BTreeSet<String>> =
            Vec::with_capacity(event_ids.len());

        for group in &event_ids {
            let mut group_chain = BTreeSet::new();

            let mut stack: Vec<&str> =
                group.into_iter().map(|e| e.as_ref()).collect();
            let mut new_stack = Vec::new();

            while !stack.is_empty() {
                for event_id in &stack {
                    if group_chain.contains(*event_id) {
                        continue;
                    }

                    if let Some(event) = store.event_map.get(*event_id) {
                        new_stack.extend(event.auth_event_ids().into_iter());
                        group_chain.insert(event.event_id().to_string());
                    } else {
                        return future::err(format_err!(
                            "Don't have full auth chain"
                        ))
                        .boxed_local();
                    }
                }
                swap(&mut stack, &mut new_stack);
                new_stack.clear();
            }

            auth_chains.push(group_chain);
        }

        let union = auth_chains.iter().fold(BTreeSet::new(), |u, x| {
            x.union(&u).map(|e| e.to_string()).collect()
        });
        let intersection = auth_chains.iter().fold(union.clone(), |u, x| {
            x.intersection(&u).map(|e| e.to_string()).collect()
        });

        let differences = union.difference(&intersection);

        let events = differences
            .into_iter()
            .filter_map(|e| store.event_map.get(e))
            .cloned()
            .collect();

        future::ok(events).boxed_local()
    }
}
