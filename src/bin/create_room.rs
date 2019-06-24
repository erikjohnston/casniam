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
use casniam::protocol::{Event, RoomVersion, RoomVersion4};
use casniam::stores::memory;

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

    let create = EventBuilder::new(
        room_id.clone(),
        creator.clone(),
        "m.room.create".to_string(),
        Some("".to_string()),
    )
    .with_content(to_value(json!({
        "creator": creator,
    })))
    .build::<R, _>(&database)
    .await?;

    let member = EventBuilder::new(
        room_id,
        creator.clone(),
        "m.room.member".to_string(),
        Some(creator.clone()),
    )
    .with_prev_events(vec![create.event_id().to_string()])
    .with_content(to_value(json!({
        "membership": "join",
    })))
    .build::<R, _>(&database)
    .await?;

    Ok(HttpResponse::Ok().json(json!({ "events": vec![create, member] })))
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
