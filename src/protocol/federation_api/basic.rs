use futures_util::future::FutureExt;
use log::{error, info};
use serde_json::{json, Value};
use sodiumoxide::crypto::sign::SecretKey;

use std::collections::BTreeMap;

use crate::protocol::events::EventBuilder;
use crate::protocol::{
    DagChunkFragment, Event, Handler, RoomVersion, RoomVersion3, RoomVersion4,
};
use crate::state_map::StateMap;
use crate::stores::StoreFactory;

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
        _room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async move {
            let event_id = event.event_id().to_string();

            let _event_origin =
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
        event_ids: Vec<String>,
        limit: usize,
    ) -> BoxFuture<FederationResult<BackfillResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();

        async move {
            let pdus = event_store.get_backfill(event_ids, limit).await?;

            Ok(BackfillResponse { pdus })
        }
        .boxed()
    }

    fn on_send(
        &self,
        txn: TransactionRequest,
    ) -> BoxFuture<FederationResult<TransactionResponse>> {
        // TODO: Check against origin.

        let version_store = self.stores.get_room_version_store();

        async move {
            let mut version_to_event_map: BTreeMap<_, Vec<Value>> =
                BTreeMap::new();

            for event in txn.pdus {
                if let Some(room_id) =
                    event.get("room_id").and_then(Value::as_str)
                {
                    if let Some(room_version_id) =
                        version_store.get_room_version(room_id).await?
                    {
                        version_to_event_map
                            .entry(room_version_id)
                            .or_default()
                            .push(event);
                    } else {
                        info!("Got event for unknwon room_id")
                    }
                } else {
                    info!("Got event without a room_id")
                }
            }

            for (room_version_id, events) in version_to_event_map.into_iter() {
                match room_version_id {
                    RoomVersion3::VERSION => {
                        self.handle_incoming_events::<RoomVersion3>(events)
                            .await?
                    }
                    RoomVersion4::VERSION => {
                        self.handle_incoming_events::<RoomVersion4>(events)
                            .await?
                    }
                    r => error!("Unrecognized room version: {}", r),
                }
            }

            Ok(TransactionResponse)
        }
        .boxed()
    }
}

impl<F> StandardFederationAPI<F>
where
    F: StoreFactory<StateMap<String>> + Sized + Send + Sync + Clone + 'static,
{
    async fn handle_incoming_events<R: RoomVersion>(
        &self,
        raw_events: Vec<Value>,
    ) -> FederationResult<()> {
        let mut events = Vec::new();

        for raw_event in raw_events.into_iter() {
            if let Ok(event) = serde_json::from_value::<R::Event>(raw_event) {
                events.push(event);
            } else {
                info!("Got invalid event");
            }
        }

        let room_store = self.stores.get_room_store::<R>();
        let event_store = self.stores.get_event_store::<R>();

        let chunks = DagChunkFragment::from_events(events);
        let handler = Handler::new(self.stores.clone());

        for chunk in chunks {
            let room_id = chunk.events[0].room_id().to_string();

            if room_store
                .get_forward_extremities(room_id.clone())
                .await?
                .is_empty()
            {
                // Ignore events for rooms we're not in for now.
                info!("Ignoring events for unknown room {}", &room_id);
                continue;
            }

            let stuff = handler.handle_chunk::<R>(chunk.clone()).await?;

            event_store
                .insert_events(
                    stuff
                        .iter()
                        .map(|info| {
                            (info.event.clone(), info.state_before.clone())
                        })
                        .collect(),
                )
                .await?;

            room_store
                .insert_new_events(
                    stuff.iter().map(|i| i.event.clone()).collect(),
                )
                .await?;

            for info in &stuff {
                info!(
                    "Stored event {} from {}",
                    info.event.event_id(),
                    info.event.sender()
                );
            }
        }

        Ok(())
    }
}
