#![feature(await_macro, async_await)]

use std::collections::BTreeMap;

use actix_web::{web, App, HttpServer};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use sodiumoxide::crypto::sign;

use casniam::protocol::server_keys::KeyServerServlet;

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
