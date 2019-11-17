use anymap::any::Any;
use futures_util::future::FutureExt;
use serde_json::json;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::protocol::events::EventBuilder;
use crate::protocol::{RoomState, RoomVersion};
use crate::state_map::StateMap;
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

pub struct StandardFederationAPI<F> {
    stores: F,
}

impl<F> FederationAPI for StandardFederationAPI<F>
where
    F: StoreFactory<StateMap<String>>,
{
    fn on_make_join<R: RoomVersion>(
        &self,
        room_id: String,
        user_id: String,
    ) -> BoxFuture<FederationResult<MakeJoinResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async {
            let prev_event_ids = room_store
                .get_forward_extremities(room_id)
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
            // .origin(self.server_name.clone())
            .build(event_store)
            .await?;

            // event.sign(
            //     self.server_name.clone(),
            //     self.key_id.clone(),
            //     &self.secret_key,
            // );

            Ok(MakeJoinResponse {
                event: event,
                room_version: R::VERSION,
            })
        }
        .boxed()

        // let last_event_id = stuff.last().unwrap().event.event_id().to_string();

        // let prev_events = vec![last_event_id];

        // let mut event = EventBuilder::new(
        //     room_id,
        //     user_id.clone(),
        //     "m.room.member".to_string(),
        //     Some(user_id.clone()),
        // )
        // .with_content(to_value(json!({
        //     "membership": "join",
        // })))
        // .with_prev_events(prev_events)
        // .origin(self.server_name.clone())
        // .build(&database)
        // .await?;

        // event.sign(
        //     self.server_name.clone(),
        //     self.key_id.clone(),
        //     &self.secret_key,
        // );
    }

    fn on_send_join<R: RoomVersion>(
        &self,
        room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_backfill<R: RoomVersion>(
        &self,
        room_id: String,
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<FederationResult<BackfillResponse<R::Event>>> {
        unimplemented!()
    }

    fn on_send(
        &self,
        txn: TransactionRequest,
    ) -> BoxFuture<FederationResult<TransactionResponse>> {
        unimplemented!()
    }
}
