use crate::protocol::{Event, RoomState, RoomStateResolver, RoomVersion};
use crate::stores::{
    EventStore, RoomStore, RoomVersionStore, StateStore, StoreFactory,
};
use crate::StateMapWithData;

use anymap::{any::Any, Map};
use async_trait::async_trait;
use failure::Error;

use std::collections::{BTreeMap, BTreeSet};
use std::default;
use std::mem;
use std::sync::{Arc, RwLock};

#[derive(Default, Debug)]
pub struct MemoryEventStoreInner<R: RoomVersion, S: RoomState<String>> {
    event_map: BTreeMap<String, R::Event>,
    state_map: BTreeMap<String, S>,

    forward_extremities: BTreeMap<String, BTreeSet<String>>,
    backward_edges: BTreeMap<String, BTreeSet<String>>,
}

pub struct MemoryEventStore<R: RoomVersion, S: RoomState<String>>(
    Arc<RwLock<MemoryEventStoreInner<R, S>>>,
);

pub fn new_memory_store<R: RoomVersion, S: RoomState<String>>(
) -> MemoryEventStore<R, S> {
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
    S: RoomState<String> + Send + Sync + 'static,
{
    pub fn get_all_events(&self) -> Vec<R::Event> {
        let store = self.0.read().expect("Mutex poisoned");

        store.event_map.values().cloned().collect()
    }
}

impl<R, S> Clone for MemoryEventStore<R, S>
where
    R: RoomVersion,
    S: RoomState<String>,
{
    fn clone(&self) -> MemoryEventStore<R, S> {
        MemoryEventStore(self.0.clone())
    }
}

#[async_trait]
impl<R, S> EventStore<R> for MemoryEventStore<R, S>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState<String>,
{
    async fn insert_events(&self, events: Vec<R::Event>) -> Result<(), Error> {
        let mut store = self.0.write().expect("Mutex poisoned");

        for event in events {
            store.event_map.insert(event.event_id().to_string(), event);
        }

        Ok(())
    }

    async fn missing_events(
        &self,
        event_ids: &[&str],
    ) -> Result<Vec<String>, Error> {
        let store = self.0.read().expect("Mutex poisoned");

        Ok(event_ids
            .iter()
            .filter(|e| !store.event_map.contains_key(**e))
            .map(|&e| e.to_string())
            .collect())
    }

    async fn get_events(
        &self,
        event_ids: &[&str],
    ) -> Result<Vec<R::Event>, Error> {
        let store = self.0.read().expect("Mutex poisoned");

        Ok(event_ids
            .iter()
            .filter_map(|e| store.event_map.get(*e))
            .cloned()
            .collect())
    }
}

#[async_trait]
impl<R, S> StateStore<R, S> for MemoryEventStore<R, S>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState<String> + Send + Sync + 'static,
{
    async fn insert_state<'a>(
        &'a self,
        event: &R::Event,
        state: &'a mut S,
    ) -> Result<(), Error> {
        let mut store = self.0.write().expect("Mutex poisoned");

        store
            .state_map
            .insert(event.event_id().to_string(), state.clone());

        Ok(())
    }

    async fn get_state_before(
        &self,
        event_id: &str,
    ) -> Result<Option<S>, Error> {
        let store = self.0.read().expect("Mutex poisoned");

        let state = store.state_map.get(event_id).cloned();

        Ok(state)
    }

    async fn get_state_after(
        &self,
        event_ids: &[&str],
    ) -> Result<Option<S>, Error> {
        let mut states: Vec<S> = Vec::with_capacity(event_ids.len());

        {
            let store = self.0.read().expect("Mutex poisoned");
            for e_id in event_ids {
                if let (Some(state), Some(event)) =
                    (store.state_map.get(*e_id), store.event_map.get(*e_id))
                {
                    let mut state_ids: S = state.clone();

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
                    return Ok(None);
                }
            }
        }

        let store = self.clone();

        let state = R::State::resolve_state(states, &store).await?;

        Ok(Some(state))
    }
}

#[async_trait]
impl<R, S> RoomStore<R::Event> for MemoryEventStore<R, S>
where
    R: RoomVersion + 'static,
    R::Event: 'static,
    S: RoomState<String> + 'static,
{
    async fn insert_new_events(
        &self,
        events: Vec<R::Event>,
    ) -> Result<(), Error> {
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

        Ok(())
    }

    async fn get_forward_extremities(
        &self,
        room_id: String,
    ) -> Result<BTreeSet<String>, Error> {
        let store = self.0.read().expect("Mutex poisoned");

        let extrems = store
            .forward_extremities
            .get(&room_id)
            .cloned()
            .unwrap_or_default();

        Ok(extrems)
    }
}

#[derive(Default, Debug)]
pub struct MemoryRoomVersionStore {
    room_version_map: RwLock<BTreeMap<String, &'static str>>,
}

#[async_trait]
impl RoomVersionStore for MemoryRoomVersionStore {
    async fn get_room_version(
        &self,
        room_id: &str,
    ) -> Result<Option<&'static str>, Error> {
        let map = self.room_version_map.read().expect("room version map");

        Ok(map.get(room_id).copied())
    }

    async fn set_room_version<'a>(
        &'a self,
        room_id: &'a str,
        version: &'static str,
    ) -> Result<(), Error> {
        let mut map = self.room_version_map.write().expect("room version map");
        map.insert(room_id.to_string(), version);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct MemoryStoreFactory {
    stores: Arc<RwLock<Map<dyn Any + Sync + Send>>>,
    room_version_store: Arc<MemoryRoomVersionStore>,
}

impl MemoryStoreFactory {
    pub fn new() -> MemoryStoreFactory {
        MemoryStoreFactory {
            stores: Arc::new(RwLock::new(Map::new())),
            room_version_store: default::Default::default(),
        }
    }

    pub fn get_memory_store<R: RoomVersion>(
        &self,
    ) -> MemoryEventStore<R, StateMapWithData<String>> {
        let stores = self.stores.read().expect("Mutex poisoned");

        if let Some(store) =
            stores.get::<MemoryEventStore<R, StateMapWithData<String>>>()
        {
            return store.clone();
        }

        mem::drop(stores);

        let mut stores = self.stores.write().expect("Mutex poisoned");

        stores
            .entry::<MemoryEventStore<R, StateMapWithData<String>>>()
            .or_insert_with(new_memory_store)
            .clone()
    }
}

impl Default for MemoryStoreFactory {
    fn default() -> MemoryStoreFactory {
        MemoryStoreFactory::new()
    }
}

impl StoreFactory<StateMapWithData<String>> for MemoryStoreFactory {
    fn get_event_store<R: RoomVersion>(&self) -> Arc<dyn EventStore<R>> {
        Arc::new(self.get_memory_store::<R>())
    }

    fn get_state_store<R: RoomVersion>(
        &self,
    ) -> Arc<dyn StateStore<R, StateMapWithData<String>>> {
        Arc::new(self.get_memory_store::<R>())
    }

    fn get_room_store<R: RoomVersion>(&self) -> Arc<dyn RoomStore<R::Event>> {
        Arc::new(self.get_memory_store::<R>())
    }

    fn get_room_version_store(&self) -> Arc<dyn RoomVersionStore> {
        self.room_version_store.clone()
    }
}
