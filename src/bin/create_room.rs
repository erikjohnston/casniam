#![allow(clippy::type_complexity)]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex};

use failure::Error;
use failure::ResultExt as _;
use futures::compat::Future01CompatExt;
use futures::{FutureExt, TryFutureExt};
use futures_util::try_stream::TryStreamExt;
use http_service::Body;
use http_service::HttpService;
use hyper::server::accept::Accept;
use log::info;
use percent_encoding::percent_decode_str;
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use sodiumoxide::crypto::sign;
use tide;
use tide::error::ResultExt as _;
use tide::querystring::ContextExt as _;
use tokio::net::TcpListener;

use casniam::protocol::client::MemoryTransactionSender;
use casniam::protocol::client::TransactionSender;
use casniam::protocol::events::EventBuilder;
use casniam::protocol::server_keys::KeyServerServlet;
use casniam::protocol::server_resolver::{MatrixConnector, MatrixResolver};
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
struct AppData {
    server_name: String,
    key_id: String,
    secret_key: sign::SecretKey,
    room_databases: Arc<Mutex<anymap::Map<dyn anymap::any::Any + Send>>>,
    federation_sender: MemoryTransactionSender,
    key_server_servlet: KeyServerServlet,
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
    ) -> Result<http::Response<Body>, Error>
    where
        R: RoomVersion,
        R::Event: Serialize + DeserializeOwned,
    {
        let database = self.get_database::<R>();

        let chunks = DagChunkFragment::from_events(txn.pdus.to_vec());
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

        Ok(tide::response::json(json!({})))
    }

    /// Called `/backfill` requests
    async fn get_backfill<R>(
        self,
        event_ids: Vec<String>,
        limit: usize,
    ) -> Result<http::Response<Body>, Error>
    where
        R: RoomVersion,
        R::Event: Serialize,
    {
        let events = self
            .get_database::<R>()
            .get_backfill(event_ids, limit)
            .await?;

        Ok(tide::response::json(json!({
            "pdus": events,
        })))
    }

    async fn make_join<R>(
        self,
        room_id: String,
        user_id: String,
    ) -> Result<http::Response<Body>, Error>
    where
        R: RoomVersion + Send,
        R::Event: Serialize + Send,
    {
        let database = self.get_database::<R>();

        let stuff = self.clone().generate_room::<R>(room_id.clone()).await?;

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

        Ok(tide::response::json(json!({
            "room_version": R::VERSION,
            "event": event,
        })))
    }

    async fn send_join<R>(
        self,
        room_id: String,
        event: R::Event,
    ) -> Result<http::Response<Body>, Error>
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

        let resp = tide::response::json(json!([200, {
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

            //let chunk = self.clone().generate_chunk::<R>(room_id.clone(), [builder]).await?;

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

            tokio_timer::delay_for(std::time::Duration::from_secs(30)).await;

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
        .boxed();

        tokio::spawn(send_fut.map(|_: Result<_, Error>| ()));

        Ok(resp)
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

        let mut prev_events = database.get_events(&prev_event_ids).await?;

        let mut state = database.get_state_for(&prev_event_ids).await?.unwrap();

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

        let handler = Handler::new(database.clone());
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

    async fn generate_room<R>(
        self,
        room_id: String,
    ) -> Result<Vec<PersistEventInfo<R, StateMap<String>>>, Error>
    where
        R: RoomVersion + Send,
        R::Event: Serialize + Send,
    {
        let creator = format!("@alice:{}", &self.server_name);

        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
            .timestamp_millis() as u64;

        let builders = vec![
            EventBuilder::new(&room_id, &creator, "m.room.create", Some(""))
                .origin_server_ts(yesterday)
                .with_content(to_value(json!({
                    "room_version": R::VERSION,
                    "creator": creator,
                }))),
            EventBuilder::new(
                &room_id,
                &creator,
                "m.room.member",
                Some(&creator),
            )
            .origin_server_ts(yesterday)
            .with_content(to_value(json!({
                "membership": "join",
                "displayname": "Alice",
            }))),
            EventBuilder::new(
                &room_id,
                &creator,
                "m.room.power_levels",
                Some(""),
            )
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
            EventBuilder::new(
                &room_id,
                &creator,
                "m.room.join_rules",
                Some(""),
            )
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

        let chunk = self
            .clone()
            .generate_chunk::<R, _>(room_id, builders)
            .await?;

        let stuff = self.clone().handle_chunk::<R>(chunk).await?;

        Ok(stuff)
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
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

    let room_databases = Arc::new(Mutex::new(anymap::Map::new()));

    let (resolver, fut) = MatrixResolver::new().unwrap();
    tokio::spawn(fut.map(|_| ()));

    let federation_sender = {
        let mut ssl_builder =
            openssl::ssl::SslConnector::builder(openssl::ssl::SslMethod::tls())
                .unwrap();

        ssl_builder.set_verify(openssl::ssl::SslVerifyMode::NONE);

        let client = hyper::Client::builder()
            .build(MatrixConnector::with_resolver(resolver));

        MemoryTransactionSender {
            client,
            server_name: server_name.clone(),
            key_name: key_id.clone(),
            secret_key: secret_key.clone(),
        }
    };

    let app_data = AppData {
        server_name,
        key_id,
        secret_key,
        federation_sender,
        key_server_servlet,
        room_databases,
    };

    let key_render = |ctx: tide::Context<AppData>| {
        async move {
            let body = ctx.state().key_server_servlet.make_body();
            tide::response::json(body)
        }
    };

    let mut app = tide::App::with_state(app_data);

    app.middleware(tide::middleware::RootLogger::new());

    app.at("/_matrix/key/v2/server").get(key_render);
    app.at("/_matrix/key/v2/server/:key*").get(key_render);

    app.at("/_matrix/federation/v1/make_join/:room_id/:user_id")
        .get(|ctx: tide::Context<AppData>| {
            async move {
                let state = ctx.state().clone();
                let room_id: String = ctx.param("room_id").client_err()?;
                let user_id: String = ctx.param("user_id").client_err()?;

                let room_id = percent_decode_str(&room_id)
                    .decode_utf8()
                    .client_err()?
                    .into_owned();

                let user_id = percent_decode_str(&user_id)
                    .decode_utf8()
                    .client_err()?
                    .into_owned();

                state
                    .make_join::<RoomVersion4>(room_id, user_id)
                    .await
                    .compat()
                    .server_err()
            }
        });

    app.at("/_matrix/federation/v1/send_join/:room_id/:event_id")
        .put(|mut ctx: tide::Context<AppData>| {
            async move {
                let room_id: String = ctx.param("room_id").client_err()?;
                let event = ctx.body_json().await.client_err()?;
                let state = ctx.state();

                let room_id = percent_decode_str(&room_id)
                    .decode_utf8()
                    .client_err()?
                    .into_owned();

                state
                    .clone()
                    .send_join::<RoomVersion4>(room_id, event)
                    .await
                    .compat()
                    .server_err()
            }
        });

    app.at("/_matrix/federation/v1/send/:txn_id").put(
        |mut ctx: tide::Context<AppData>| {
            async move {
                let txn = ctx.body_json().await.client_err()?;
                let state = ctx.state();

                state
                    .clone()
                    .on_received_transaction::<RoomVersion4>(txn)
                    .await
                    .compat()
                    .server_err()
            }
        },
    );

    app.at("/_matrix/federation/v1/backfill/:room_id").get(
        |ctx: tide::Context<AppData>| {
            async move {
                let state = ctx.state();

                let mut event_ids = Vec::new();
                let mut limit = 100;

                for (key, value) in ctx.url_query::<Vec<(String, String)>>()? {
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

                info!("Got backfill request with ids: {:?}", event_ids);

                state
                    .clone()
                    .get_backfill::<RoomVersion4>(event_ids, limit)
                    .await
                    .compat()
                    .server_err()
            }
        },
    );

    app.at("/_matrix/federation/v1/query/directory").get(
        |ctx: tide::Context<AppData>| {
            async move {
                let state = ctx.state();

                let query: RoomAliasQuery = ctx.url_query()?;

                let room_id = format!(
                    "!{}:{}",
                    base64::encode_config(&Sha256::digest(query.room_alias.as_bytes()), base64::URL_SAFE_NO_PAD),
                    state.server_name.clone(),
                );

                Ok(tide::response::json(json!({ "room_id": room_id, "servers": &[&state.server_name] }))) as Result<_, tide::Error>
            }
        },
    );

    // .bind_ssl("127.0.0.1:9999", ssl_builder)?

    // app.serve("127.0.0.1:8088")

    let mut file = File::open("identity.pfx").unwrap();
    let mut identity = vec![];
    file.read_to_end(&mut identity).unwrap();
    let identity =
        native_tls::Identity::from_pkcs12(&identity, "test").unwrap();
    let acceptor = native_tls::TlsAcceptor::new(identity).unwrap();
    let acceptor = tokio_tls::TlsAcceptor::from(acceptor);

    let mut listener = TcpListener::bind("127.0.0.1:9999").await?;

    let http_config = hyper::server::conn::Http::new();

    let tide_server = app.into_http_service();

    let new_service = move || {
        let tide_server = tide_server.clone();

        hyper::service::service_fn(
            move |mut req: http::Request<hyper::Body>| -> futures::future::BoxFuture<
                Result<http::Response<hyper::Body>, failure::Error>,
            > {
                let s = tide_server.clone();

                async move {
                    let mut vec: Vec<u8> = Vec::new();
                    while let Some(next) = req.body_mut().next().await {
                        let chunk = next?;
                        vec.extend(chunk);
                    }

                    let req = req.map(|_| Body::from(vec));
                    let resp = s.respond(&mut (), req).await?;

                    let (parts, body) = resp.into_parts();
                    let resp_body = body.into_vec().await?;

                    Ok(http::Response::from_parts(
                        parts,
                        hyper::Body::from(resp_body),
                    ))
                }
                .boxed()
            },
        )
    };

    loop {
        let (stream, remote_addr) = listener.accept().await.unwrap();
        let tls_stream = acceptor.accept(stream).await.unwrap();

        let fut = http_config.serve_connection(tls_stream, new_service());

        tokio::spawn(fut.map(|_| ()));
    }
}
