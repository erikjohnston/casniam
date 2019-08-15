#![feature(async_await, async_closure)]
#![allow(clippy::type_complexity)]

use std::collections::BTreeMap;

use actix_rt::Arbiter;
use actix_web::{self, web, App, HttpResponse, HttpServer};
use failure::Error;
use futures::{compat, FutureExt};
use log::info;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde::Serialize;
use serde_json::json;
use sodiumoxide::crypto::sign;

use std::sync::{Arc, Mutex};

use casniam::protocol::client::MemoryTransactionSender;
use casniam::protocol::client::TransactionSender;
use casniam::protocol::events::EventBuilder;
use casniam::protocol::server_keys::KeyServerServlet;
use casniam::protocol::{
    DagChunkFragment, Event, Handler, PersistEventInfo, RoomVersion,
    RoomVersion4,
};
use casniam::state_map::StateMap;
use casniam::stores::{memory, EventStore};

fn to_value(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(value) => value,
        _ => panic!("Expected json map"),
    }
}

async fn generate_room<R>(
    room_id: String,
    server_name: String,
    key_name: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R>,
) -> Result<Vec<PersistEventInfo<R, StateMap<String>>>, Error>
where
    R: RoomVersion,
    R::Event: Serialize,
{
    let creator = format!("@alice:{}", server_name);

    let handler = Handler::new(database.clone());

    let mut chunk = DagChunkFragment::new();

    let builders = vec![
        EventBuilder::new(&room_id, &creator, "m.room.create", Some(""))
            .with_content(to_value(json!({
                "room_version": R::VERSION,
                "creator": creator,
            }))),
        EventBuilder::new(&room_id, &creator, "m.room.member", Some(&creator))
            .with_content(to_value(json!({
                "membership": "join",
            }))),
        EventBuilder::new(&room_id, &creator, "m.room.power_levels", Some(""))
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
            .with_content(to_value(json!({
                "join_rule": "public",
            }))),
        EventBuilder::new(
            &room_id,
            &creator,
            "m.room.message",
            None as Option<String>,
        )
        .with_content(to_value(json!({
            "msgtype": "m.text",
            "body": "Are you there?",
        }))),
    ];

    for builder in builders {
        let prev_events: Vec<_> =
            chunk.forward_extremities().iter().cloned().collect();

        let state = database.get_state_for(&prev_events).await?.unwrap();

        let mut event = builder
            .with_prev_events(prev_events)
            .origin(server_name.clone())
            .build::<R, _>(&database)
            .await?;

        event.sign(server_name.clone(), key_name.clone(), &secret_key);

        database.insert_event(event.clone(), state.clone());
        chunk.add_event(event).unwrap();
    }

    let stuff = handler.handle_chunk::<R>(chunk).await?;

    for info in &stuff {
        assert!(!info.rejected);
    }

    Ok(stuff)
}

async fn render_room<R>(
    server_name: String,
    key_name: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R>,
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
        room_id,
        server_name,
        key_name,
        secret_key,
        database.clone(),
    )
    .await?;

    let last_event_id = stuff.last().unwrap().event.event_id();
    let state = database.get_state_for(&[last_event_id]).await?.unwrap();

    Ok(HttpResponse::Ok().json(json!({
        "events": stuff.into_iter().map(|p| p.event).collect::<Vec<_>>(),
        "state": state.values().cloned().collect::<Vec<_>>(),
    })))
}

async fn make_join<R>(
    room_id: String,
    user_id: String,
    server_name: String,
    key_name: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R>,
) -> Result<HttpResponse, Error>
where
    R: RoomVersion,
    R::Event: Serialize,
{
    let stuff = generate_room(
        room_id.clone(),
        server_name.clone(),
        key_name.clone(),
        secret_key.clone(),
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
    .origin(server_name.clone())
    .build::<R, _>(&database)
    .await?;

    event.sign(server_name.clone(), key_name.clone(), &secret_key);

    Ok(HttpResponse::Ok().json(json!({
        "room_version": R::VERSION,
        "event": event,
    })))
}

async fn send_join<R>(
    room_id: String,
    event: R::Event,
    server_name: String,
    key_name: String,
    secret_key: sign::SecretKey,
    database: memory::MemoryEventStore<R>,
) -> Result<HttpResponse, Error>
where
    R: RoomVersion,
    R::Event: Serialize,
{
    let event_id = event.event_id().to_string();

    let event_origin =
        event.sender().splitn(2, ':').last().unwrap().to_string();

    let chunk = DagChunkFragment::from_event(event.clone());
    let handler = Handler::new(database.clone());
    let mut stuff = handler.handle_chunk::<R>(chunk).await?;

    for info in &mut stuff {
        assert!(!info.rejected);

        info.event
            .sign(server_name.clone(), key_name.clone(), &secret_key);
        database.insert_event(info.event.clone(), info.state_before.clone());
    }

    let state = database.get_state_for(&[&event_id]).await?.unwrap();
    let state_events = database.get_events(state.values()).await?;

    let resp = HttpResponse::Ok().json(json!([200, {
        "origin": server_name,
        "state": state_events.clone(),
        "auth_chain": state_events,
    }]));

    let prev_events = vec![event_id.clone()];
    let send_fut = async move {
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

        let sender = MemoryTransactionSender {
            client,
            server_name: server_name.clone(),
            key_name: key_name.clone(),
            secret_key: secret_key.clone(),
        };

        sender
            .send_event::<R>(event_origin.clone(), event)
            .await
            .unwrap();

        let creator = format!("@alice:{}", server_name);

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

        let state =
            database.get_state_for(&prev_events).await.unwrap().unwrap();

        let mut event = builder
            .with_prev_events(prev_events)
            .origin(server_name.clone())
            .build::<R, _>(&database)
            .await
            .unwrap();

        event.sign(server_name.clone(), key_name.clone(), &secret_key);

        database.insert_event(event.clone(), state.clone());

        info!("Sending event to {}", event_origin);

        sender
            .send_event::<R>(event_origin.clone(), event.clone())
            .await
            .unwrap();

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
            database.get_state_for(&prev_events).await.unwrap().unwrap();

        let mut event = builder
            .with_prev_events(prev_events)
            .origin(server_name.clone())
            .build::<R, _>(&database)
            .await
            .unwrap();

        event.sign(server_name.clone(), key_name.clone(), &secret_key);

        database.insert_event(event.clone(), state.clone());

        compat::Compat01As03::new(tokio_timer::Delay::new(std::time::Instant::now() + std::time::Duration::from_millis(500))).await.unwrap();

        info!("Sending event to {}", event_origin);

        sender.send_event::<R>(event_origin, event).await.unwrap();

        Ok(())
    }
        .boxed_local();

    Arbiter::spawn(compat::Compat::new(send_fut));

    Ok(resp)
}

async fn get_backfill<R>(
    _room_id: String,
    event_ids: Vec<String>,
    limit: usize,
    database: memory::MemoryEventStore<R>,
) -> Result<HttpResponse, Error>
where
    R: RoomVersion,
    R::Event: Serialize,
{
    let events = database.get_backfill(event_ids, limit).await?;

    Ok(HttpResponse::Ok().json(json!({
        "pdus": events,
    })))
}

#[derive(Clone)]
struct AppData {
    server_name: String,
    key_id: String,
    secret_key: sign::SecretKey,
    room_databases: Arc<Mutex<anymap::Map<dyn anymap::any::Any + Send>>>,
}

impl AppData {
    fn get_database<R: RoomVersion>(&self) -> memory::MemoryEventStore<R> {
        let mut map = self.room_databases.lock().unwrap();
        map.entry().or_insert_with(memory::new_memory_store).clone()
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

    let app_data = AppData {
        server_name,
        key_id,
        secret_key,
        room_databases: Arc::new(Mutex::new(anymap::Map::new())),
    };

    HttpServer::new(move || {
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
                    compat::Compat::new(
                        render_room(
                            app_data.server_name.clone(),
                            app_data.key_id.clone(),
                            app_data.secret_key.clone(),
                            app_data.get_database::<RoomVersion4>(), // FIXME
                        )
                        .boxed_local(),
                    )
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
                        compat::Compat::new(
                            make_join(
                                path.0.clone(),
                                path.1.clone(),
                                app_data.server_name.clone(),
                                app_data.key_id.clone(),
                                app_data.secret_key.clone(),
                                app_data.get_database::<RoomVersion4>(), // FIXME
                            )
                            .boxed_local(),
                        )
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
                        compat::Compat::new(
                            send_join(
                                path.0.clone(),
                                event.0,
                                app_data.server_name.clone(),
                                app_data.key_id.clone(),
                                app_data.secret_key.clone(),
                                app_data.get_database::<RoomVersion4>(), // FIXME
                            )
                            .boxed_local(),
                        )
                    },
                )),
            )
            .service(
                web::resource("/_matrix/federation/v1/send/{txn_id}").route(
                    web::put().to(move || HttpResponse::Ok().json(json!({}))),
                ),
            )
            .service(
                web::resource("/_matrix/federation/v1/backfill/{room_id}")
                    .route(web::get().to_async(
                        move |(path, query, app_data): (
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

                            compat::Compat::new(
                                get_backfill(
                                    path.0.clone(),
                                    event_ids,
                                    limit,
                                    app_data.get_database::<RoomVersion4>(), // FIXME
                                )
                                .boxed_local(),
                            )
                        },
                    )),
            )
    })
    .bind("127.0.0.1:8088")?
    .bind_ssl("127.0.0.1:9999", ssl_builder)?
    .run()
}
