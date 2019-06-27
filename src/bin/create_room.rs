#![feature(await_macro, async_await)]

use std::collections::BTreeMap;

use actix_web::{self, web, App, HttpResponse, HttpServer};
use failure::Error;
use futures::{compat, FutureExt};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde::Serialize;
use serde_json::json;
use sodiumoxide::crypto::sign;

use casniam::protocol::events::EventBuilder;
use casniam::protocol::server_keys::KeyServerServlet;
use casniam::protocol::{
    DagChunkFragment, Event, Handler, RoomVersion, RoomVersion4,
};
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
    server_name: String,
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

    let creator = format!("@alice:{}", server_name);

    let handler = Handler::new(database.clone());

    let mut chunk = DagChunkFragment::new();

    let builders = vec![
        EventBuilder::new(
            room_id.clone(),
            creator.clone(),
            "m.room.create".to_string(),
            Some("".to_string()),
        )
        .with_content(to_value(json!({
            "creator": creator,
        }))),
        EventBuilder::new(
            room_id,
            creator.clone(),
            "m.room.member".to_string(),
            Some(creator.clone()),
        )
        .with_content(to_value(json!({
            "membership": "join",
        }))),
    ];

    for builder in builders {
        let prev_events = chunk
            .forward_extremities()
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        let state = database.get_state_for(&prev_events).await?.unwrap();

        let event = builder
            .with_prev_events(prev_events)
            .origin(server_name.clone())
            .build::<R, _>(&database)
            .await?;
        database.insert_event(event.clone(), state.clone());
        chunk.add_event(event).unwrap();
    }

    let stuff = handler.handle_chunk::<R>(chunk).await?;

    for info in &stuff {
        assert!(!info.rejected);
    }

    let last_event_id = stuff.last().unwrap().event.event_id();

    let state = database.get_state_for(&[last_event_id]).await?.unwrap();

    Ok(HttpResponse::Ok().json(json!({
        "events": stuff.into_iter().map(|p| p.event).collect::<Vec<_>>(),
        "state": state.values().cloned().collect::<Vec<_>>(),
    })))
}

fn main() -> std::io::Result<()> {
    let server_name = "localhost:9999".to_string();

    let (pubkey, seckey) = sign::gen_keypair();
    let key_id = format!(
        "ed25519:{}",
        thread_rng()
            .sample_iter(&Alphanumeric)
            .take(5)
            .collect::<String>()
    );

    let mut verify_keys = BTreeMap::new();
    verify_keys.insert(key_id, (pubkey, seckey));

    let database = memory::new_memory_store::<RoomVersion4>();

    let key_server_servlet = KeyServerServlet::new(
        server_name.clone(),
        verify_keys,
        BTreeMap::new(),
    );

    HttpServer::new(move || {
        let server_name = server_name.clone();
        let database = database.clone();
        let key_server_servlet = key_server_servlet.clone();
        App::new()
            .service(
                web::resource("/_matrix/key/v2/server*")
                    .route(web::get().to(move || key_server_servlet.render())),
            )
            .service(web::resource("/create_room").route(web::get().to_async(
                move || {
                    compat::Compat::new(
                        generate_room(server_name.clone(), database.clone())
                            .boxed_local(),
                    )
                },
            )))
    })
    .bind("127.0.0.1:8088")?
    .run()
}
