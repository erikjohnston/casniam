use futures_util::future::FutureExt;
use log::info;
use serde_json::{json, Value};
use sodiumoxide::crypto::sign::SecretKey;

use std::collections::{BTreeMap, HashMap};

use crate::protocol::events::EventBuilder;
use crate::protocol::{
    DagChunkFragment, Event, Handler, PersistEventInfo, RoomState, RoomVersion, RoomVersion3,
    RoomVersion4, RoomVersion5,
};
use crate::stores::StoreFactory;
use crate::StateMapWithData;

use super::*;

fn to_value(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(value) => value,
        _ => panic!("Expected json map"),
    }
}

pub trait Hooks: Sync + Send + 'static {
    fn on_new_events<R: RoomVersion>(
        &self,
        _infos: &[PersistEventInfo<R, StateMapWithData<String>>],
    ) -> BoxFuture<FederationResult<()>> {
        async { Ok(()) }.boxed()
    }

    fn on_send_join<R: RoomVersion>(&self, _user_id: String) -> BoxFuture<FederationResult<()>> {
        async { Ok(()) }.boxed()
    }
}

impl Hooks for () {}

#[derive(Clone, Debug)]
pub struct StandardFederationAPI<F, H = ()> {
    stores: F,
    server_name: String,
    key_id: String,
    secret_key: SecretKey,
    hooks: H,
    handler: Handler<StateMapWithData<String>, F>,
}

impl<F, H> StandardFederationAPI<F, H> {
    pub fn hook(&self) -> &H {
        &self.hooks
    }

    pub fn handler(&self) -> &Handler<StateMapWithData<String>, F> {
        &self.handler
    }
}

impl<F, H> FederationAPI for StandardFederationAPI<F, H>
where
    F: StoreFactory<StateMapWithData<String>> + Sized + Send + Sync + Clone + 'static,
    H: Hooks,
{
    fn on_make_join<R: RoomVersion>(
        &self,
        _origin: String,
        room_id: String,
        user_id: String,
    ) -> BoxFuture<FederationResult<MakeJoinResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();
        let state_store = self.stores.get_state_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async move {
            let prev_event_ids: Vec<_> = room_store
                .get_forward_extremities(room_id.clone())
                .await?
                .into_iter()
                .collect();

            if prev_event_ids.is_empty() {
                return Err(FederationAPIError::HttpResponse(
                    http::Response::builder()
                        .status(404)
                        .header("Content-Type", "application/json")
                        .body(b"{}".to_vec())
                        .expect("valid http response"),
                ));
            }

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
            .build(event_store.as_ref(), state_store.as_ref())
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
        origin: String,
        room_id: String,
        event: R::Event,
    ) -> BoxFuture<FederationResult<SendJoinResponse<R::Event>>> {
        let event_store = self.stores.get_event_store::<R>();
        let state_store = self.stores.get_state_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async move {
            if event.event_type() != "m.room.member" {
                return Err(format_err!("Event is not a membership event").into());
            }

            if event.state_key().is_none() {
                return Err(format_err!("Event is not a state event").into());
            };

            if event
                .content()
                .get("membership")
                .and_then(serde_json::Value::as_str)
                != Some("join")
            {
                return Err(format_err!("Event does not have membership join").into());
            }

            let event_id = event.event_id().to_string();

            let mut stuff = self
                .handler
                .handle_new_timeline_events::<R>(&origin, &room_id, vec![event.clone()])
                .await?;

            for info in &mut stuff {
                assert!(!info.rejected);

                info.event.sign(
                    self.server_name.clone(),
                    self.key_id.clone(),
                    &self.secret_key,
                );

                room_store.insert_new_event(info.event.clone()).await?;
            }

            self.hooks
                .on_send_join::<R>(event.sender().to_string())
                .await?;

            let state = expect_or_err!(state_store.get_state_before(&event_id).await?);

            let state_events = event_store
                .get_events(&state.values().map(|e| e as &str).collect::<Vec<_>>())
                .await?;

            let auth_events = event_store
                .get_auth_chain_ids(state.values().map(String::to_string).collect::<Vec<_>>())
                .await?;

            Ok(SendJoinResponse {
                state: state_events,
                auth_chain: auth_events,
            })
        }
        .boxed()
    }

    fn on_backfill<R: RoomVersion>(
        &self,
        _origin: String,
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
        origin: String,
        txn: TransactionRequest,
    ) -> BoxFuture<FederationResult<TransactionResponse>> {
        // TODO: Check against origin.

        let version_store = self.stores.get_room_version_store();

        async move {
            let mut version_to_event_map: BTreeMap<_, Vec<Value>> = BTreeMap::new();

            for event in txn.pdus {
                if let Some(room_id) = event.get("room_id").and_then(Value::as_str) {
                    if let Some(room_version_id) = version_store.get_room_version(room_id).await? {
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
                        self.handle_incoming_events::<RoomVersion3>(&origin, events)
                            .await?
                    }
                    RoomVersion4::VERSION => {
                        self.handle_incoming_events::<RoomVersion4>(&origin, events)
                            .await?
                    }
                    RoomVersion5::VERSION => {
                        self.handle_incoming_events::<RoomVersion5>(&origin, events)
                            .await?
                    }
                    r => info!("Unrecognized room version: {}", r),
                }
            }

            Ok(TransactionResponse)
        }
        .boxed()
    }
}

impl<F, H> StandardFederationAPI<F, H>
where
    F: StoreFactory<StateMapWithData<String>> + Sized + Send + Sync + Clone + 'static,
    H: Hooks,
{
    pub fn new(
        stores: F,
        server_name: String,
        key_id: String,
        secret_key: SecretKey,
        hooks: H,
        handler: Handler<StateMapWithData<String>, F>,
    ) -> StandardFederationAPI<F, H> {
        StandardFederationAPI {
            stores,
            server_name,
            key_id,
            secret_key,
            hooks,
            handler,
        }
    }

    async fn handle_incoming_events<R: RoomVersion>(
        &self,
        origin: &str,
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

        let chunks = DagChunkFragment::from_events(events);

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

            let stuff = self
                .handler
                .handle_new_timeline_events::<R>(origin, &room_id, chunk.into_events())
                .await?;

            room_store
                .insert_new_events(stuff.iter().map(|i| i.event.clone()).collect())
                .await?;

            self.hooks.on_new_events(&stuff).await?;

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

    /// Handles the response to a send join.
    pub async fn handle_join_room_version<R: RoomVersion>(
        &self,
        destination: &str,
        event: R::Event,
        response: SendJoinResponse<R::Event>,
    ) -> Result<(), Error> {
        // let event_store = self.stores.get_event_store::<R>();
        // let state_store = self.stores.get_state_store::<R>();
        let room_store = self.stores.get_room_store::<R>();
        let room_version_store = self.stores.get_room_version_store();

        let room_id = event.room_id();

        room_version_store
            .set_room_version(room_id, R::VERSION)
            .await?;

        let handler = self.handler();

        let mut all_events = response.state.clone();
        all_events.extend_from_slice(&response.auth_chain);
        all_events.push(event.clone());

        let len_all_events = all_events.len();

        let persisted_events = handler
            .check_auth_auth_chain_and_persist::<R>(destination, room_id, all_events)
            .await?;

        if persisted_events.len() != len_all_events {
            todo!()
        }

        let mut event_to_state = HashMap::new();
        for prev_event_id in event.prev_event_ids() {
            event_to_state.insert(
                prev_event_id.to_string(),
                response
                    .state
                    .iter()
                    .filter_map(|e| {
                        e.state_key().map(|state_key| {
                            (
                                (e.event_type().to_string(), state_key.to_string()),
                                e.event_id().to_string(),
                            )
                        })
                    })
                    .collect(),
            );
        }

        handler
            .handle_chunk::<R>(DagChunkFragment::from_event(event.clone()), event_to_state)
            .await?;

        room_store.insert_new_events(vec![event.clone()]).await?;

        Ok(())
    }
}
