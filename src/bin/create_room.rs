#![allow(clippy::type_complexity)]

use std::collections::BTreeMap;

use failure::Error;
use futures::future::BoxFuture;
use futures::FutureExt;
use log::{error, info};
use percent_encoding::percent_decode_str;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use sodiumoxide::crypto::sign;

use casniam::protocol::client::MemoryTransactionSender;
use casniam::protocol::client::TransactionSender;
use casniam::protocol::events::EventBuilder;
use casniam::protocol::federation_api::{
    basic::Hooks, basic::StandardFederationAPI, FederationAPI,
    FederationResult, TransactionRequest,
};
use casniam::protocol::server_keys::KeyServerServlet;
use casniam::protocol::server_resolver::{MatrixConnector, MatrixResolver};
use casniam::protocol::{
    DagChunkFragment, Event, Handler, PersistEventInfo, RoomState, RoomVersion,
    RoomVersion3, RoomVersion4,
};
use casniam::state_map::StateMap;
use casniam::stores::{memory, EventStore, RoomStore, StoreFactory};

#[derive(Serialize, Deserialize)]
struct RoomAliasQuery {
    room_alias: String,
}

#[derive(Serialize, Deserialize)]
struct Transaction<R: RoomVersion>
where
    R::Event: Serialize + DeserializeOwned,
{
    pdus: Vec<R::Event>,
}

#[derive(Clone)]
struct BasicHooks {
    server_name: String,
    key_id: String,
    secret_key: sign::SecretKey,
    stores: memory::MemoryStoreFactory,
    federation_sender: MemoryTransactionSender,
}

impl Hooks for BasicHooks {
    fn on_new_events<R: RoomVersion>(
        &self,
        infos: &[PersistEventInfo<R, StateMap<String>>],
    ) -> BoxFuture<FederationResult<()>> {
        let infos = infos.to_vec();

        let event_store = self.stores.get_event_store::<R>();
        let room_store = self.stores.get_room_store::<R>();

        async move {
            for info in infos {
                if info.event.event_type() == "m.room.message" {
                    let room_id = info.event.room_id().to_string();

                    let extrems: Vec<_> = room_store
                        .get_forward_extremities(room_id.clone())
                        .await?
                        .into_iter()
                        .collect();

                    let creator = format!("@alice:{}", self.server_name);
                    let event_origin = info
                        .event
                        .sender()
                        .splitn(2, ':')
                        .last()
                        .unwrap()
                        .to_string();

                    let state = event_store
                        .get_state_for(
                            &extrems
                                .iter()
                                .map(|e| e as &str)
                                .collect::<Vec<_>>(),
                        )
                        .await?
                        .unwrap();

                    let mut event = EventBuilder::from_json(json!({
                        "room_id": room_id,
                        "sender": creator,
                        "type": "m.room.message",
                        "content": {
                            "msgtype": "m.text",
                            "body": "Why are you talking to me??",
                        },
                        "prev_events": extrems,
                    }))?
                    .build(event_store.as_ref())
                    .await?;

                    event.sign(
                        self.server_name.clone(),
                        self.key_id.clone(),
                        &self.secret_key,
                    );

                    event_store
                        .insert_event(event.clone(), state.clone())
                        .await?;

                    room_store.insert_new_event(event.clone()).await?;

                    info!("Sending echo event to {}", event_origin);

                    self.federation_sender
                        .send_event::<R>(event_origin, event)
                        .await
                        .unwrap();
                }
            }
            Ok(())
        }
        .boxed()
    }
}

/// Takes a `str` room version and calls the expression with a type alias `R` set
/// to the appopriate type.
#[macro_export]
macro_rules! route_room_version {
    ($ver:expr, $f:expr) => {
        match $ver {
            RoomVersion3::VERSION => {
                type R = RoomVersion3;
                $f
            }
            RoomVersion4::VERSION => {
                type R = RoomVersion4;
                $f
            }
            _ => {
                error!("Unrecognized version {}", $ver);
                return Err(actix_web::error::ErrorInternalServerError(
                    "Unknown version",
                ));
            }
        }
    };
}

#[derive(Clone)]
struct AppData {
    server_name: String,
    key_id: String,
    secret_key: sign::SecretKey,
    stores: memory::MemoryStoreFactory,
    federation_sender: MemoryTransactionSender,
    key_server_servlet: KeyServerServlet,
    federation_api:
        StandardFederationAPI<memory::MemoryStoreFactory, BasicHooks>,
}

impl AppData {
    fn get_database<R: RoomVersion>(
        &self,
    ) -> memory::MemoryEventStore<R, StateMap<String>> {
        self.stores.get_memory_store::<R>()
    }

    async fn send_some_events<R>(
        self,
        room_id: String,
        remote: String,
    ) -> Result<(), Error>
    where
        R: RoomVersion,
        R::Event: Serialize,
    {
        let database = self.get_database::<R>();

        let creator = format!("@alice:{}", self.server_name);

        let prev_events: Vec<_> = database
            .get_forward_extremities(room_id.clone())
            .await?
            .into_iter()
            .collect();

        let mut event = EventBuilder::from_json(json!({
            "room_id": room_id,
            "sender": creator,
            "type": "m.room.message",
            "content": {
                "msgtype": "m.text",
                "body": "Hello! I don't actually have anything to say to you right now...",
            },
            "prev_events": prev_events,
        }))?
        .build(&database)
        .await?;

        event.sign(
            self.server_name.clone(),
            self.key_id.clone(),
            &self.secret_key,
        );

        let state = database
            .get_state_for(
                &prev_events.iter().map(|e| e as &str).collect::<Vec<_>>(),
            )
            .await?
            .unwrap();

        database.insert_event(event.clone(), state.clone()).await?;

        database.insert_new_event(event.clone()).await?;

        info!("Sending 'hello' event to {}", remote);

        self.federation_sender
            .send_event::<R>(remote.clone(), event.clone())
            .await?;

        tokio::time::delay_for(std::time::Duration::from_secs(30)).await;

        let prev_events = vec![event.event_id().to_string()];

        let mut event = EventBuilder::from_json(json!({
            "room_id": room_id,
            "sender": &creator,
            "type": "m.room.member",
            "state_key": &creator,
            "content": {
                "membership": "leave",
            },
            "prev_events": prev_events,
        }))?
        .build(&database)
        .await?;

        event.sign(
            self.server_name.clone(),
            self.key_id.clone(),
            &self.secret_key,
        );

        let state = database
            .get_state_for(
                &prev_events.iter().map(|e| e as &str).collect::<Vec<_>>(),
            )
            .await?
            .unwrap();

        database.insert_event(event.clone(), state.clone()).await?;

        database.insert_new_event(event.clone()).await?;

        info!("Sending 'leave' event to {}", remote);

        self.federation_sender
            .send_event::<R>(remote, event)
            .await?;

        Ok(())
    }

    /// Create a new chunk from a set of builders. Doesn't persist.
    async fn generate_chunk<
        R: RoomVersion + Send,
        B: IntoIterator<Item = EventBuilder> + Send,
    >(
        self,
        room_id: String,
        builders: B,
    ) -> Result<DagChunkFragment<R::Event>, Error> {
        let database = self.get_database::<R>();

        let mut chunk = DagChunkFragment::new();
        let mut prev_event_ids: Vec<_> = database
            .get_forward_extremities(room_id)
            .await?
            .into_iter()
            .collect();

        let mut prev_events = database
            .get_events(
                &prev_event_ids.iter().map(|e| e as &str).collect::<Vec<_>>(),
            )
            .await?;

        let mut state = database
            .get_state_for(
                &prev_event_ids.iter().map(|e| e as &str).collect::<Vec<_>>(),
            )
            .await?
            .unwrap();

        for builder in builders {
            info!("Prev events for new chunk: {:?}", &prev_event_ids);

            let mut event = R::Event::from_builder::<R, _>(
                builder
                    .with_prev_events(prev_event_ids)
                    .origin(self.server_name.clone()),
                state.clone(),
                prev_events.clone(),
            )
            .await?;

            event.sign(
                self.server_name.clone(),
                self.key_id.clone(),
                &self.secret_key,
            );
            chunk.add_event(event.clone()).unwrap();

            if let Some(state_key) = event.state_key() {
                state.add_event(
                    event.event_type().to_string(),
                    state_key.to_string(),
                    event.event_id().to_string(),
                );
            }

            prev_event_ids = vec![event.event_id().to_string()];
            prev_events = vec![event];
        }

        Ok(chunk)
    }

    async fn handle_chunk<R>(
        self,
        chunk: DagChunkFragment<R::Event>,
    ) -> Result<Vec<PersistEventInfo<R, StateMap<String>>>, Error>
    where
        R: RoomVersion,
        R::Event: Serialize,
    {
        let database = self.get_database::<R>();

        let handler = Handler::new(self.stores.clone());
        let stuff = handler.handle_chunk::<R>(chunk).await?;

        for info in &stuff {
            assert!(!info.rejected);
        }

        database.insert_events(
            stuff
                .iter()
                .map(|info| (info.event.clone(), info.state_before.clone()))
                .collect(),
        );

        // TODO: Do we need to clone?
        database
            .insert_new_events(
                stuff.iter().map(|info| info.event.clone()).collect(),
            )
            .await?;

        Ok(stuff)
    }

    async fn generate_room<R>(
        self,
        room_id: String,
    ) -> Result<Vec<PersistEventInfo<R, StateMap<String>>>, Error>
    where
        R: RoomVersion + Send,
        R::Event: Serialize + Send,
    {
        let creator = format!("@alice:{}", &self.server_name);

        info!("Generating room for {}", room_id);

        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
            .timestamp_millis() as u64;

        self.stores
            .get_room_version_store()
            .set_room_version(&room_id, R::VERSION)
            .await?;

        let builders = vec![
            json!({
                "room_id": &room_id,
                "sender": &creator,
                "origin_server_ts": yesterday,
                "type": "m.room.create",
                "state_key": "",
                "content": {
                    "creator": &creator,
                    "room_version": R::VERSION,
                },
            }),
            json!({
                "room_id": &room_id,
                "sender": &creator,
                "origin_server_ts": yesterday,
                "type": "m.room.member",
                "state_key": &creator,
                "content": {
                    "membership": "join",
                    "displayname": "Alice",
                },
            }),
            json!({
                "room_id": &room_id,
                "sender": &creator,
                "origin_server_ts": yesterday,
                "type": "m.room.power_levels",
                "state_key": "",
                "content": {
                    "users": {
                        &creator: 100,
                    },
                    "users_default": 100,
                    "events": {},
                    "events_default": 0,
                    "state_default": 50,
                    "ban": 50,
                    "kick": 50,
                    "redact": 50,
                    "invite": 0
                },
            }),
            json!({
                "room_id": &room_id,
                "sender": &creator,
                "origin_server_ts": yesterday,
                "type": "m.room.join_rules",
                "state_key": "",
                "content": {
                    "join_rule": "public",
                },
            }),
            json!({
                "room_id": &room_id,
                "sender": &creator,
                "origin_server_ts": yesterday,
                "type": "m.room.message",
                "content": {
                    "msgtype": "m.text",
                    "body": "Are you there?",
                },
            }),
        ];

        let chunk = self
            .clone()
            .generate_chunk::<R, _>(
                room_id,
                builders
                    .into_iter()
                    .map(|j| EventBuilder::from_json(j).expect("valid json")),
            )
            .await?;

        let stuff = self.clone().handle_chunk::<R>(chunk).await?;

        Ok(stuff)
    }
}

fn add_routes(cfg: &mut actix_web::web::ServiceConfig) {
    cfg.route(
        "/_matrix/key/v2/server",
        actix_web::web::get().to(
            |app_data: actix_web::web::Data<AppData>| async move {
                let body = app_data.key_server_servlet.make_body();

                Ok(actix_web::web::Json(body)) as actix_web::Result<_>
            },
        ),
    )
    .route(
        "/_matrix/key/v2/server/{key}",
        actix_web::web::get().to(
            |app_data: actix_web::web::Data<AppData>| async move {
                let body = app_data.key_server_servlet.make_body();

                Ok(actix_web::web::Json(body)) as actix_web::Result<_>
            },
        ),
    )
    .route(
        "/_matrix/federation/v1/make_join/{room_id}/{user_id}",
        actix_web::web::get().to(
            |(state, path): (
                actix_web::web::Data<AppData>,
                actix_web::web::Path<(String, String)>,
            )| {
                async move {
                    let app_data: &AppData = &state;

                    let room_id =
                        percent_decode_str(&path.0).decode_utf8()?.into_owned();

                    let user_id =
                        percent_decode_str(&path.1).decode_utf8()?.into_owned();

                    let room_version_opt: Option<&'static str> = app_data.stores.get_room_version_store().get_room_version(&room_id).await?;
                    let room_version = if let Some(room_version) = room_version_opt {
                        room_version
                    } else {
                        return Err(actix_web::error::ErrorNotFound("Unknown room"))
                    };

                    // TODO: Check if remote server supports room version via ?ver= params

                    let response = route_room_version!(
                        room_version,
                        serde_json::to_value(
                            app_data
                            .federation_api
                            .on_make_join::<R>(room_id, user_id)
                            .await
                            .unwrap()
                        ).unwrap() // FIXME
                    );

                    Ok(actix_web::web::Json(response)) as actix_web::Result<_>
                }
            },
        ),
    )
    .route(
        "/_matrix/federation/v2/send_join/{room_id}/{event_id}",
        actix_web::web::put().to(
            |(state, path, body): (
                actix_web::web::Data<AppData>,
                actix_web::web::Path<(String, String)>,
                actix_web::web::Json<serde_json::Value>,
            )| {
                async move {
                    let app_data: AppData = state.as_ref().clone();

                    let room_id =
                        percent_decode_str(&path.0).decode_utf8()?.into_owned();

                    let room_version_opt: Option<&'static str> = app_data.stores.get_room_version_store().get_room_version(&room_id).await?;
                    let room_version = if let Some(room_version) = room_version_opt {
                        room_version
                    } else {
                        return Err(actix_web::error::ErrorNotFound("Unknown room"))
                    };

                    let response = route_room_version!(
                        room_version,
                        {
                            let event: <R as RoomVersion>::Event =
                                serde_json::from_value(body.0)?;

                            let event_origin = event
                                .sender()
                                .splitn(2, ':')
                                .last()
                                .unwrap()
                                .to_string();

                            app_data.federation_sender
                                .send_event::<R>(event_origin.clone(), event.clone())
                                .await
                                .unwrap();

                            let response = app_data
                                .federation_api
                                .on_send_join::<R>(room_id.clone(), event)
                                .await
                                .unwrap(); // FIXME

                            tokio::spawn(async move{
                                app_data.send_some_events::<R>(room_id, event_origin).await.ok();
                            });

                            serde_json::to_value(response).unwrap()
                        }
                    );

                    Ok(actix_web::web::Json(response)) as actix_web::Result<_>
                }
            },
        ),
    )
    .route(
        "/_matrix/federation/v1/send/{txn_id}",
        actix_web::web::put().to(
            |(state, _path, body): (
                actix_web::web::Data<AppData>,
                actix_web::web::Path<(String,)>,
                actix_web::web::Json<serde_json::Value>,
            )| {
                async move {
                    let app_data: &AppData = &state;

                    let transaction: TransactionRequest =
                        serde_json::from_value(body.0)?;

                    app_data
                        .federation_api
                        .on_send(transaction)
                        .await
                        .unwrap(); // FIXME

                    Ok(actix_web::web::Json(json!({}))) as actix_web::Result<_>
                }
            },
        ),
    )
    .route(
        "/_matrix/federation/v1/query/directory",
        actix_web::web::get().to(
            |(state, query): (
                actix_web::web::Data<AppData>,
                actix_web::web::Query<RoomAliasQuery>,
            )| {
                async move {
                    let app_data: &AppData = &state;

                    let room_id = format!(
                        "!{}:{}",
                        base64::encode_config(
                            &Sha256::digest(query.room_alias.as_bytes()),
                            base64::URL_SAFE_NO_PAD
                        ),
                        state.server_name.clone(),
                    );

                    if app_data.stores.get_room_version_store().get_room_version(&room_id).await.unwrap().is_none() {
                        // Now create the room if it doesn't exist.
                        app_data
                            .clone()
                            .generate_room::<RoomVersion4>(room_id.clone())
                            .await?;
                    }

                    Ok(actix_web::web::Json(json!({ "room_id": room_id, "servers": &[&app_data.server_name] }))) as actix_web::Result<_>
                }
            },
        ),
    ).route(
        "/_matrix/federation/v1/backfill/{room_id}",
        actix_web::web::get().to(
            |(state, path, query): (
                actix_web::web::Data<AppData>,
                actix_web::web::Path<(String,)>,
                actix_web::web::Query<Vec<(String, String)>>,
            )| {
                async move {
                    let app_data: &AppData = &state;
                    let room_id = path.0.clone();

                    let mut event_ids = Vec::new();
                    let mut limit = 100;

                    for (key, value) in query.iter() {
                        match key as &str {
                            "v" => event_ids.push(value.clone()),
                            "limit" => {
                                if let Ok(l) = value.clone().parse() {
                                    limit = l
                                }
                            }
                            _ => {}
                        }
                    }

                    let room_version_opt: Option<&'static str> = app_data.stores.get_room_version_store().get_room_version(&room_id).await?;
                    let room_version = if let Some(room_version) = room_version_opt {
                        room_version
                    } else {
                        return Err(actix_web::error::ErrorNotFound("Unknown room"))
                    };

                    let response =route_room_version!(
                        room_version,
                        {
                            let response = app_data
                                .federation_api
                                .on_backfill::<R>(room_id, event_ids, limit)
                                .await
                                .unwrap(); // FIXME

                            serde_json::to_value(response).unwrap()
                        }
                    );

                    Ok(actix_web::web::Json(response)) as actix_web::Result<_>
                }
            },
        ),
    );
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    // let _guard = slog_envlogger::init().unwrap();

    let server_name = "localhost:9999".to_string();

    let (pubkey, secret_key) = sign::gen_keypair();
    let key_id = format!(
        "ed25519:{}",
        thread_rng()
            .sample_iter(&Alphanumeric)
            .take(5)
            .collect::<String>()
    );

    let mut verify_keys = BTreeMap::new();
    verify_keys.insert(key_id.clone(), (pubkey, secret_key.clone()));

    let key_server_servlet = KeyServerServlet::new(
        server_name.clone(),
        verify_keys,
        BTreeMap::new(),
    );

    let stores = memory::MemoryStoreFactory::new();

    let resolver = MatrixResolver::new().await.unwrap();

    let federation_sender = {
        let client = hyper::Client::builder()
            .build(MatrixConnector::with_resolver(resolver));

        MemoryTransactionSender {
            client,
            server_name: server_name.clone(),
            key_name: key_id.clone(),
            secret_key: secret_key.clone(),
        }
    };

    let hooks = BasicHooks {
        server_name: server_name.clone(),
        key_id: key_id.clone(),
        secret_key: secret_key.clone(),
        federation_sender: federation_sender.clone(),
        stores: stores.clone(),
    };

    let federation_api = StandardFederationAPI::new(
        stores.clone(),
        server_name.clone(),
        key_id.clone(),
        secret_key.clone(),
        hooks,
    );

    let app_data = AppData {
        server_name,
        key_id,
        secret_key,
        federation_sender,
        key_server_servlet,
        stores,
        federation_api,
    };

    let http_server = actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .data(app_data.clone())
            .app_data(app_data.clone())
            .wrap(actix_web::middleware::Logger::default())
            .configure(add_routes)
    })
    .bind("127.0.0.1:9998")
    .unwrap();

    let local = tokio::task::LocalSet::new();
    let fut = actix_rt::System::run_in_tokio("casniam", &local);
    local.spawn_local(fut);
    local
        .run_until(async move { http_server.run().await })
        .await?;

    Ok(())
}
