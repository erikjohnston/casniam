use futures_util::future::FutureExt;
use serde_json::json;
use sodiumoxide::crypto::sign::SecretKey;

use anymap::AnyMap;

use crate::protocol::events::EventBuilder;
use crate::protocol::{Event, RoomState, RoomVersion};
use crate::state_map::StateMap;
use crate::stores::memory::MemoryEventStore;
use crate::stores::{EventStore, RoomStore};

use super::*;

fn to_value(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(value) => value,
        _ => panic!("Expected json map"),
    }
}

pub trait StoreFactory<S: RoomState> {
    fn get_event_store<R: RoomVersion>(&self) -> &dyn EventStore<R, S>;
    fn get_room_store<R: RoomVersion>(&self) -> &dyn RoomStore<R::Event>;
}

#[derive(Debug)]
pub struct MemoryStoreFactory {
    stores: AnyMap,
}

impl MemoryStoreFactory {
    pub fn new() -> MemoryStoreFactory {
        MemoryStoreFactory {
            stores: AnyMap::new(),
        }
    }

    pub fn add<R: RoomVersion>(
        &mut self,
        store: MemoryEventStore<R, StateMap<String>>,
    ) {
        self.stores.insert(store);
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
    ) -> &dyn EventStore<R, StateMap<String>> {
        self.stores
            .get::<MemoryEventStore<R, StateMap<String>>>()
            .unwrap()
    }

    fn get_room_store<R: RoomVersion>(&self) -> &dyn RoomStore<R::Event> {
        self.stores
            .get::<MemoryEventStore<R, StateMap<String>>>()
            .unwrap()
    }
}

pub struct StandardFederationAPI<F> {
    stores: F,
    server_name: String,
    key_id: String,
    secret_key: SecretKey,
}

impl<F> FederationAPI for StandardFederationAPI<F>
where
    F: StoreFactory<StateMap<String>> + Sized + Send + Sync,
{
    fn on_make_join<R: RoomVersion>(
        &self,
        room_id: String,
        user_id: String,
    ) -> BoxFuture<FederationResult<MakeJoinResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async move {
            let prev_event_ids = room_store
                .get_forward_extremities(room_id.clone())
                .await?
                .into_iter()
                .collect();

            let mut event = EventBuilder::new(
                room_id,
                user_id.clone(),
                "m.room.member".to_string(),
                Some(user_id.clone()),
            )
            .with_content(to_value(json!({
                "membership": "join",
            })))
            .with_prev_events(prev_event_ids)
            .origin(self.server_name.clone())
            .build(event_store)
            .await?;

            event.sign(
                self.server_name.clone(),
                self.key_id.clone(),
                &self.secret_key,
            );

            Ok(MakeJoinResponse {
                event,
                room_version: R::VERSION,
            })
        }
        .boxed()
    }

    fn on_send_join<R: RoomVersion>(
        &self,
        _room_id: String,
        _event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_backfill<R: RoomVersion>(
        &self,
        _room_id: String,
        _event_ids: Vec<String>,
        _limit: usize,
    ) -> BoxFuture<FederationResult<BackfillResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_send(
        &self,
        _txn: TransactionRequest,
    ) -> BoxFuture<FederationResult<TransactionResponse>> {
        unimplemented!()
    }
}
