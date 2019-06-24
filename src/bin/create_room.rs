#![feature(await_macro, async_await)]

use std::collections::BTreeMap;

use actix_web::{web, App, HttpServer};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use serde_json::json;
use sodiumoxide::crypto::sign;

use casniam::protocol::events::EventBuilder;
use casniam::protocol::server_keys::KeyServerServlet;
use casniam::protocol::{RoomVersion, RoomVersion4};
use casniam::stores::memory;

async fn generate_room<R: RoomVersion>(
    server_name: String,
    database: &memory::MemoryEventStore<R>,
) {
    let room_id = format!(
        "!{}:{}",
        thread_rng()
            .sample_iter(&Alphanumeric)
            .take(10)
            .collect::<String>(),
        server_name,
    );

    let creator = format!("@alice:{}", server_name);

    let mut builder = EventBuilder::new(
        room_id,
        creator.clone(),
        "m.room.create".to_string(),
        Some("".to_string()),
    );

    builder.with_content(
        if let serde_json::Value::Object(a) = json!({
            "creator": creator,
        }) {
            a
        } else {
            unreachable!()
        },
    );

    builder.build_v2(database).await;
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

    let key_server_servlet =
        KeyServerServlet::new(server_name, verify_keys, BTreeMap::new());

    HttpServer::new(move || {
        let key_server_servlet = key_server_servlet.clone();
        App::new().service(
            web::resource("/_matrix/key/v2/server*")
                .to(move || key_server_servlet.render()),
        )
    })
    .bind("127.0.0.1:8088")?
    .run()
}
