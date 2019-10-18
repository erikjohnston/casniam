#![allow(clippy::type_complexity)]

use std::collections::BTreeMap;

use actix_rt::Arbiter;
use actix_web::{self, web, App, HttpResponse, HttpServer};
use failure::Error;
use futures::compat::Future01CompatExt;
use futures::{FutureExt, TryFutureExt};
use log::info;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sodiumoxide::crypto::sign;

use std::sync::{Arc, Mutex};

use casniam::protocol::client::MemoryTransactionSender;
use casniam::protocol::client::TransactionSender;
use casniam::protocol::events::EventBuilder;
use casniam::protocol::server_keys::KeyServerServlet;
use casniam::protocol::{
    DagChunkFragment, Event, Handler, PersistEventInfo, RoomState, RoomVersion,
    RoomVersion4,
};
use casniam::state_map::StateMap;
use casniam::stores::{memory, EventStore, RoomStore};

fn to_value(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(value) => value,
        _ => panic!("Expected json map"),
    }
}

async fn generate_chunk<R: RoomVersion>(
    room_id: String,
    server_name: String,
    key_id: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R, StateMap<String>>,
    builders: impl IntoIterator<Item = EventBuilder>,
) -> Result<DagChunkFragment<R::Event>, Error> {
    let mut chunk = DagChunkFragment::new();
    let mut prev_event_ids: Vec<_> = database
        .get_forward_extremities(room_id)
        .await?
        .into_iter()
        .collect();

    let mut prev_events = database.get_events(&prev_event_ids).await?;

    let mut state = database.get_state_for(&prev_event_ids).await?.unwrap();

    for builder in builders {
        info!("Prev events for new chunk: {:?}", &prev_event_ids);

        let mut event = R::Event::from_builder::<R, _>(
            builder
                .with_prev_events(prev_event_ids)
                .origin(server_name.clone()),
            state.clone(),
            prev_events.clone(),
        )
        .await?;

        event.sign(server_name.clone(), key_id.clone(), &secret_key);
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

async fn generate_room<R>(
    room_id: String,
    server_name: String,
    key_name: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R, StateMap<String>>,
) -> Result<Vec<PersistEventInfo<R, StateMap<String>>>, Error>
where
    R: RoomVersion,
    R::Event: Serialize,
{
    let creator = format!("@alice:{}", server_name);

    let handler = Handler::new(database.clone());

    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
        .timestamp_millis() as u64;

    let builders = vec![
        EventBuilder::new(&room_id, &creator, "m.room.create", Some(""))
            .origin_server_ts(yesterday)
            .with_content(to_value(json!({
                "room_version": R::VERSION,
                "creator": creator,
            }))),
        EventBuilder::new(&room_id, &creator, "m.room.member", Some(&creator))
            .origin_server_ts(yesterday)
            .with_content(to_value(json!({
                "membership": "join",
                "displayname": "Alice",
            }))),
        EventBuilder::new(&room_id, &creator, "m.room.power_levels", Some(""))
            .origin_server_ts(yesterday)
            .with_content(to_value(json!({
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
            }))),
        EventBuilder::new(&room_id, &creator, "m.room.join_rules", Some(""))
            .origin_server_ts(yesterday)
            .with_content(to_value(json!({
                "join_rule": "public",
            }))),
        EventBuilder::new(
            &room_id,
            &creator,
            "m.room.message",
            None as Option<String>,
        )
        .origin_server_ts(yesterday)
        .with_content(to_value(json!({
            "msgtype": "m.text",
            "body": "Are you there?",
        }))),
    ];

    let chunk = generate_chunk(
        room_id.clone(),
        server_name.clone(),
        key_name.clone(),
        secret_key.clone(),
        database.clone(),
        builders.clone(),
    )
    .await?;

    let stuff = handler.handle_chunk(chunk).await?;

    for info in &stuff {
        assert!(!info.rejected);
    }

    database.insert_events(
        stuff
            .iter()
            .map(|info| (info.event.clone(), info.state_before.clone())),
    );

    // TODO: Do we need to clone?
    database
        .insert_new_events(stuff.iter().map(|info| info.event.clone()))
        .await?;

    Ok(stuff)
}

async fn render_room<R>(
    server_name: String,
    key_name: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R, StateMap<String>>,
) -> Result<HttpResponse, Error>
where
    R: RoomVersion,
    R::Event: Serialize,
{
    let room_id = format!(
        "!{}:{}",
        thread_rng()
            .sample_iter(&Alphanumeric)
            .take(10)
            .collect::<String>(),
        server_name,
    );

    let stuff = generate_room(
        room_id.clone(),
        server_name,
        key_name,
        secret_key,
        database.clone(),
    )
    .await?;

    let extrems: Vec<_> = database
        .get_forward_extremities(room_id)
        .await?
        .into_iter()
        .collect();
    let state = database.get_state_for(&extrems).await?.unwrap();

    Ok(HttpResponse::Ok().json(json!({
        "events": stuff.into_iter().map(|p| p.event).collect::<Vec<_>>(),
        "state": state.values().cloned().collect::<Vec<_>>(),
    })))
}

#[derive(Serialize, Deserialize)]
struct Transaction<R: RoomVersion>
where
    R::Event: Serialize + DeserializeOwned,
{
    pdus: Vec<R::Event>,
}

#[derive(Clone)]
struct AppData {
    server_name: String,
    key_id: String,
    secret_key: sign::SecretKey,
    room_databases: Arc<Mutex<anymap::Map<dyn anymap::any::Any + Send>>>,
    federation_sender: MemoryTransactionSender,
}

impl AppData {
    fn get_database<R: RoomVersion>(
        &self,
    ) -> memory::MemoryEventStore<R, StateMap<String>> {
        let mut map = self.room_databases.lock().unwrap();
        map.entry().or_insert_with(memory::new_memory_store).clone()
    }

    /// Gets called on `/send/` requests
    async fn on_received_transaction<R>(
        self,
        txn: Transaction<R>,
    ) -> Result<HttpResponse, Error>
    where
        R: RoomVersion,
        R::Event: Serialize + DeserializeOwned,
    {
        let database = self.get_database::<R>();

        let chunks =
            DagChunkFragment::from_events(txn.pdus.iter().cloned().collect());
        let handler = Handler::new(database.clone());

        for chunk in chunks {
            let room_id = chunk.events[0].room_id().to_string();

            if database
                .get_forward_extremities(room_id.clone())
                .await?
                .is_empty()
            {
                // Ignore events for rooms we're not in for now.
                info!("Ignoring events for unknown room {}", &room_id);
                continue;
            }

            let stuff = handler.handle_chunk(chunk).await?;

            database
                .insert_events(stuff.iter().map(|info| {
                    (info.event.clone(), info.state_before.clone())
                }))
                .await?;

            database
                .insert_new_events(stuff.iter().map(|i| i.event.clone()))
                .await?;

            for info in stuff {
                if info.event.event_type() == "m.room.message" {
                    let room_id = info.event.room_id().to_string();

                    let extrems: Vec<_> = database
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

                    let state =
                        database.get_state_for(&extrems).await?.unwrap();
                    let builder = EventBuilder::new(
                        &room_id,
                        &creator,
                        "m.room.message",
                        None as Option<String>,
                    )
                    .with_content(to_value(json!({
                        "msgtype": "m.text",
                        "body": "Why are you talking to me??",
                    })));

                    let mut event = builder
                        .with_prev_events(extrems)
                        .origin(self.server_name.clone())
                        .build::<R, _>(&database)
                        .await?;

                    event.sign(
                        self.server_name.clone(),
                        self.key_id.clone(),
                        &self.secret_key,
                    );

                    database.insert_event(event.clone(), state.clone()).await?;

                    database.insert_new_event(event.clone()).await?;

                    info!("Sending echo event to {}", event_origin);

                    self.federation_sender
                        .send_event::<R>(event_origin, event)
                        .await
                        .unwrap();
                }
            }
        }

        Ok(HttpResponse::Ok().json(json!({})))
    }

    /// Called `/backfill` requests
    async fn get_backfill<R>(
        self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> Result<HttpResponse, Error>
    where
        R: RoomVersion,
        R::Event: Serialize,
    {
        let events = self
            .get_database::<R>()
            .get_backfill(event_ids, limit)
            .await?;

        Ok(HttpResponse::Ok().json(json!({
            "pdus": events,
        })))
    }

    async fn make_join<R>(
        self,
        room_id: String,
        user_id: String,
    ) -> Result<HttpResponse, Error>
    where
        R: RoomVersion,
        R::Event: Serialize,
    {
        let database = self.get_database::<R>();

        let stuff = generate_room(
            room_id.clone(),
            self.server_name.clone(),
            self.key_id.clone(),
            self.secret_key.clone(),
            database.clone(),
        )
        .await?;

        let last_event_id = stuff.last().unwrap().event.event_id().to_string();

        let prev_events = vec![last_event_id];

        let mut event = EventBuilder::new(
            room_id,
            user_id.clone(),
            "m.room.member".to_string(),
            Some(user_id.clone()),
        )
        .with_content(to_value(json!({
            "membership": "join",
        })))
        .with_prev_events(prev_events)
        .origin(self.server_name.clone())
        .build::<R, _>(&database)
        .await?;

        event.sign(
            self.server_name.clone(),
            self.key_id.clone(),
            &self.secret_key,
        );

        Ok(HttpResponse::Ok().json(json!({
            "room_version": R::VERSION,
            "event": event,
        })))
    }

    async fn send_join<R>(
        self,
        room_id: String,
        event: R::Event,
    ) -> Result<HttpResponse, Error>
    where
        R: RoomVersion,
        R::Event: Serialize,
    {
        let database = self.get_database::<R>();

        let event_id = event.event_id().to_string();

        let event_origin =
            event.sender().splitn(2, ':').last().unwrap().to_string();

        let chunk = DagChunkFragment::from_event(event.clone());
        let handler = Handler::new(database.clone());
        let mut stuff = handler.handle_chunk(chunk).await?;

        for info in &mut stuff {
            assert!(!info.rejected);

            info.event.sign(
                self.server_name.clone(),
                self.key_id.clone(),
                &self.secret_key,
            );

            database
                .insert_event(info.event.clone(), info.state_before.clone())
                .await?;

            database.insert_new_event(info.event.clone()).await?;
        }

        let state = database.get_state_for(&[&event_id]).await?.unwrap();
        let state_events = database.get_events(state.values()).await?;

        let resp = HttpResponse::Ok().json(json!([200, {
            "origin": self.server_name,
            "state": state_events.clone(),
            "auth_chain": state_events,
        }]));

        let send_fut = async move {
            self.federation_sender
                .send_event::<R>(event_origin.clone(), event)
                .await?;

            let creator = format!("@alice:{}", self.server_name);

            let builder = EventBuilder::new(
                &room_id,
                &creator,
                "m.room.message",
                None as Option<String>,
            )
            .with_content(to_value(json!({
                "msgtype": "m.text",
                "body": "Hello! I don't actually have anything to say to you right now...",
            })));

            let prev_events: Vec<_> = database.get_forward_extremities(room_id.clone()).await?.into_iter().collect();

            let state =
                database.get_state_for(&prev_events).await?.unwrap();

            let mut event = builder
                .with_prev_events(prev_events)
                .origin(self.server_name.clone())
                .build::<R, _>(&database)
                .await?;

            event.sign(self.server_name.clone(), self.key_id.clone(), &self.secret_key);

            database.insert_event(
                event.clone(),
                state.clone(),
            ).await?;

            database.insert_new_event(
                event.clone(),
            ).await?;

            info!("Sending 'hello' event to {}", event_origin);

            self.federation_sender
                .send_event::<R>(event_origin.clone(), event.clone())
                .await?;

            tokio_timer::Delay::new(std::time::Instant::now() + std::time::Duration::from_secs(30)).compat().await?;

            let builder = EventBuilder::new(
                &room_id,
                &creator,
                "m.room.member",
                Some(creator.clone()),
            )
            .with_content(to_value(json!({
                "membership": "leave",
            })));

            let prev_events = vec![event.event_id().to_string()];
            let state =
                database.get_state_for(&prev_events).await?.unwrap();

            let mut event = builder
                .with_prev_events(prev_events)
                .origin(self.server_name.clone())
                .build::<R, _>(&database)
                .await?;

            event.sign(self.server_name.clone(), self.key_id.clone(), &self.secret_key);

            database.insert_event(
                event.clone(),
                state.clone(),
            ).await?;

            database.insert_new_event(
                event.clone(),
            ).await?;

            info!("Sending 'leave' event to {}", event_origin);

            self.federation_sender.send_event::<R>(event_origin, event).await?;

            Ok(())
        }
        .boxed_local();

        Arbiter::spawn(send_fut.map_err(|_: Error| ()).compat());

        Ok(resp)
    }
}

fn main() -> std::io::Result<()> {
    env_logger::init();

    let server_name = "localhost:9999".to_string();

    let mut ssl_builder = openssl::ssl::SslAcceptor::mozilla_intermediate(
        openssl::ssl::SslMethod::tls(),
    )
    .unwrap();

    ssl_builder
        .set_certificate_file("cert.crt", openssl::ssl::SslFiletype::PEM)
        .unwrap();

    ssl_builder
        .set_private_key_file("cert.key", openssl::ssl::SslFiletype::PEM)
        .unwrap();

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

    let databases = Arc::new(Mutex::new(anymap::Map::new()));

    HttpServer::new(move || {
        let federation_sender = {
            let mut ssl_builder =
                openssl::ssl::SslConnector::builder(openssl::ssl::SslMethod::tls())
                    .unwrap();

            ssl_builder.set_verify(openssl::ssl::SslVerifyMode::NONE);

            let client = awc::Client::build()
                .connector(
                    actix_http::client::Connector::new()
                        .ssl(ssl_builder.build())
                        .finish(),
                )
                .finish();

            MemoryTransactionSender {
                client,
                server_name: server_name.clone(),
                key_name: key_id.clone(),
                secret_key: secret_key.clone(),
            }
        };

        let app_data = AppData {
            server_name: server_name.clone(),
            key_id: key_id.clone(),
            secret_key: secret_key.clone(),
            federation_sender,
            room_databases: databases.clone(),
        };

        let key_server_servlet = key_server_servlet.clone();

        App::new()
            .data(app_data.clone())
            .wrap(actix_web::middleware::Logger::default())
            .service(
                web::resource("/_matrix/key/v2/server*")
                    .route(web::get().to(move || key_server_servlet.render())),
            )
            .service(web::resource("/create_room").route(web::get().to_async(
                move |app_data: web::Data<AppData>| {
                    render_room(
                        app_data.server_name.clone(),
                        app_data.key_id.clone(),
                        app_data.secret_key.clone(),
                        app_data.get_database::<RoomVersion4>(), // FIXME
                    )
                    .boxed_local().compat()
                },
            )))
            .service(
                web::resource(
                    "/_matrix/federation/v1/make_join/{room_id}/{user_id}",
                )
                .route(web::get().to_async(
                    move |(path, app_data): (
                        web::Path<(String, String)>,
                        web::Data<AppData>,
                    )| {
                        app_data.get_ref().clone().make_join::<RoomVersion4>(
                            path.0.clone(),
                            path.1.clone(),
                        )
                        .boxed_local().compat()
                    },
                )),
            )
            .service(
                web::resource(
                    "/_matrix/federation/v1/send_join/{room_id}/{event_id}",
                )
                .route(web::put().to_async(
                    move |(path, app_data, event): (
                        web::Path<(String, String)>,
                        web::Data<AppData>,
                        web::Json<<RoomVersion4 as RoomVersion>::Event>, // FIXME
                    )| {
                        app_data.get_ref().clone().send_join::<RoomVersion4>(
                            path.0.clone(),
                            event.0,
                        )
                        .boxed_local().compat()
                    },
                )),
            )
            .service(
                web::resource("/_matrix/federation/v1/send/{txn_id}").route(
                    web::put().to_async(move |(txn, app_data): (web::Json<Transaction<RoomVersion4>>, web::Data<AppData>)| {
                        app_data.get_ref().clone().on_received_transaction(txn.0).boxed_local().compat()
                    }),
                ),
            )
            .service(
                web::resource("/_matrix/federation/v1/backfill/{room_id}")
                    .route(web::get().to_async(
                        move |(_path, query, app_data): (
                            web::Path<(String,)>,
                            web::Query<Vec<(String, String)>>,
                            web::Data<AppData>,
                        )| {
                            let mut event_ids = Vec::new();
                            let mut limit = 100;

                            for (key, value) in query.into_inner() {
                                match &*key {
                                    "v" => event_ids.push(value),
                                    "limit" => {
                                        if let Ok(l) = value.parse() {
                                            limit = l
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            info!(
                                "Got backfill request with ids: {:?}",
                                event_ids
                            );

                            app_data.get_ref().clone().get_backfill::<RoomVersion4>(
                                event_ids,
                                limit,
                            )
                            .boxed_local().compat()
                        },
                    )),
            )
            .service(
                web::resource("/_matrix/federation/v1/query/directory").route(
                    web::get().to(move |(app_data, query): (web::Data<AppData>, web::Query<Vec<(String, String)>>,)| {
                        let query_map: BTreeMap<_, _> = query.into_inner().into_iter().collect();

                        let room_alias = &query_map["room_alias"];

                        let room_id = format!(
                            "!{}:{}",
                            base64::encode_config(&Sha256::digest(room_alias.as_bytes()), base64::URL_SAFE_NO_PAD),
                            app_data.server_name.clone(),
                        );

                        HttpResponse::Ok().json(json!({ "room_id": room_id, "servers": &[&app_data.server_name] }))
                    }),
                ),
            )
    })
    .bind("127.0.0.1:8088")?
    .bind_ssl("127.0.0.1:9999", ssl_builder)?
    .run()
}
