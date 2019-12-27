use crate::protocol::{Event, RoomState, RoomStateResolver, RoomVersion};
use crate::state_map::StateMap;
use crate::stores::{EventStore, RoomStore, StoreFactory};

use anymap::{any::Any, Map};
use failure::Error;
use futures::future::BoxFuture;
use futures::{future, FutureExt};

use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::sync::{Arc, RwLock};

#[derive(Default, Debug)]
pub struct MemoryEventStoreInner<R: RoomVersion, S: RoomState> {
    event_map: BTreeMap<String, R::Event>,
    state_map: BTreeMap<String, S>,

    forward_extremities: BTreeMap<String, BTreeSet<String>>,
    backward_edges: BTreeMap<String, BTreeSet<String>>,
}

pub struct MemoryEventStore<R: RoomVersion, S: RoomState>(
    Arc<RwLock<MemoryEventStoreInner<R, S>>>,
);

pub fn new_memory_store<R: RoomVersion, S: RoomState>() -> MemoryEventStore<R, S>
{
    MemoryEventStore(Arc::new(RwLock::new(MemoryEventStoreInner {
        event_map: BTreeMap::new(),
        state_map: BTreeMap::new(),
        forward_extremities: BTreeMap::new(),
        backward_edges: BTreeMap::new(),
    })))
}

impl<R, S> MemoryEventStore<R, S>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState + Send + Sync + 'static,
{
    pub fn get_all_events(&self) -> Vec<R::Event> {
        let store = self.0.read().expect("Mutex poisoned");

        store.event_map.values().cloned().collect()
    }
}

impl<R, S> Clone for MemoryEventStore<R, S>
where
    R: RoomVersion,
    S: RoomState,
{
    fn clone(&self) -> MemoryEventStore<R, S> {
        MemoryEventStore(self.0.clone())
    }
}

impl<R, S> EventStore<R, S> for MemoryEventStore<R, S>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState + Send + Sync + 'static,
{
    fn insert_events(
        &self,
        events: Vec<(R::Event, S)>,
    ) -> BoxFuture<Result<(), Error>> {
        let mut store = self.0.write().expect("Mutex poisoned");

        for (event, state) in events {
            store.state_map.insert(event.event_id().to_string(), state);
            store.event_map.insert(event.event_id().to_string(), event);
        }

        future::ok(()).boxed()
    }

    fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<String>, Error>> {
        let store = self.0.read().expect("Mutex poisoned");

        future::ok(
            event_ids
                .iter()
                .filter(|e| !store.event_map.contains_key(**e))
                .map(|&e| e.to_string())
                .collect(),
        )
        .boxed()
    }

    fn get_events(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Vec<R::Event>, Error>> {
        let store = self.0.read().expect("Mutex poisoned");

        future::ok(
            event_ids
                .iter()
                .filter_map(|e| store.event_map.get(*e))
                .cloned()
                .collect(),
        )
        .boxed()
    }

    fn get_state_for(
        &self,
        event_ids: &[&str],
    ) -> BoxFuture<Result<Option<S>, Error>> {
        let mut states: Vec<S> = Vec::with_capacity(event_ids.len());

        {
            let store = self.0.read().expect("Mutex poisoned");
            for e_id in event_ids {
                if let (Some(state), Some(event)) =
                    (store.state_map.get(*e_id), store.event_map.get(*e_id))
                {
                    let mut state_ids: S = state.clone().into_iter().collect();

                    // Since we're getting the resolved state we need to add the
                    // event itself if its a state event.
                    if let Some(state_key) = event.state_key() {
                        state_ids.add_event(
                            event.event_type().to_string(),
                            state_key.to_string(),
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
            let state = R::State::resolve_state(states, &store).await?;

            Ok(Some(state))
        }
        .boxed()
    }
}

impl<R, S> RoomStore<R::Event> for MemoryEventStore<R, S>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState + 'static,
{
    fn insert_new_events(
        &self,
        events: Vec<R::Event>,
    ) -> BoxFuture<Result<(), Error>> {
        let mut store = self.0.write().expect("Mutex poisoned");

        for event in events {
            let MemoryEventStoreInner {
                ref mut forward_extremities,
                ref mut backward_edges,
                ..
            } = &mut *store;

            let extremities = forward_extremities
                .entry(event.room_id().to_string())
                .or_default();

            for prev_id in event.prev_event_ids() {
                backward_edges
                    .entry(prev_id.to_string())
                    .or_default()
                    .insert(event.event_id().to_string());

                // TODO: We want to insert the prev events' prev events here
                // too, as we can't assume that they're there already there if
                // it was rejected

                extremities.remove(prev_id);
            }

            if !backward_edges.contains_key(event.event_id()) {
                extremities.insert(event.event_id().to_string());
            }

            store.event_map.insert(event.event_id().to_string(), event);
        }

        future::ok(()).boxed()
    }

    fn get_forward_extremities(
        &self,
        room_id: String,
    ) -> BoxFuture<Result<BTreeSet<String>, Error>> {
        let store = self.0.read().expect("Mutex poisoned");

        let extrems = store
            .forward_extremities
            .get(&room_id)
            .cloned()
            .unwrap_or_default();

        future::ok(extrems).boxed()
    }
}

#[derive(Debug, Clone)]
pub struct MemoryStoreFactory {
    stores: Arc<RwLock<Map<dyn Any + Sync + Send>>>,
}

impl MemoryStoreFactory {
    pub fn new() -> MemoryStoreFactory {
        MemoryStoreFactory {
            stores: Arc::new(RwLock::new(Map::new())),
        }
    }

    pub fn get_memory_store<R: RoomVersion>(
        &self,
    ) -> MemoryEventStore<R, StateMap<String>> {
        let stores = self.stores.read().expect("Mutex poisoned");

        if let Some(store) =
            stores.get::<MemoryEventStore<R, StateMap<String>>>()
        {
            return store.clone();
        }

        mem::drop(stores);

        let mut stores = self.stores.write().expect("Mutex poisoned");

        stores
            .entry::<MemoryEventStore<R, StateMap<String>>>()
            .or_insert_with(new_memory_store)
            .clone()
    }
}

impl Default for MemoryStoreFactory {
    fn default() -> MemoryStoreFactory {
        MemoryStoreFactory::new()
    }
}

impl StoreFactory<StateMap<String>> for MemoryStoreFactory {
    fn get_event_store<R: RoomVersion>(
        &self,
    ) -> Arc<dyn EventStore<R, StateMap<String>>> {
        Arc::new(self.get_memory_store::<R>())
    }

    fn get_room_store<R: RoomVersion>(&self) -> Arc<dyn RoomStore<R::Event>> {
        Arc::new(self.get_memory_store::<R>())
    }
}
