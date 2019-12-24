use futures_util::future::FutureExt;
use serde_json::json;
use sodiumoxide::crypto::sign::SecretKey;

use crate::protocol::events::EventBuilder;
use crate::protocol::{
    DagChunkFragment, Event, Handler, RoomState, RoomVersion,
};
use crate::state_map::StateMap;
use crate::stores::{EventStore, RoomStore, StoreFactory};

use super::*;

fn to_value(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(value) => value,
        _ => panic!("Expected json map"),
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
    F: StoreFactory<StateMap<String>> + Sized + Send + Sync + Clone + 'static,
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
            .build(event_store.as_ref())
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
        room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async move {
            let event_id = event.event_id().to_string();

            let event_origin =
                event.sender().splitn(2, ':').last().unwrap().to_string();

            let chunk = DagChunkFragment::from_event(event.clone());
            let handler = Handler::new(self.stores.clone());
            let mut stuff = handler.handle_chunk::<R>(chunk).await?;

            for info in &mut stuff {
                assert!(!info.rejected);

                info.event.sign(
                    self.server_name.clone(),
                    self.key_id.clone(),
                    &self.secret_key,
                );

                event_store
                    .insert_event(info.event.clone(), info.state_before.clone())
                    .await?;

                room_store.insert_new_event(info.event.clone()).await?;
            }

            let state = event_store.get_state_for(&[&event_id]).await?.unwrap();
            let state_events = event_store
                .get_events(
                    &state.values().map(|e| e as &str).collect::<Vec<_>>(),
                )
                .await?;

            Ok(SendJoinResponse {
                origin: self.server_name.clone(),
                state: state_events.clone(),
                auth_chain: state_events,
            })
        }
        .boxed()
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
