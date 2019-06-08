#![feature(await_macro, async_await)]

use std::collections::BTreeMap;

use actix_web::{web, App, HttpResponse, HttpServer};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use sodiumoxide::crypto::sign;

fn render_server_keys() -> HttpResponse {
    // For now just generate a random key each time.
    let (pubkey, seckey) = sign::gen_keypair();
    let key_name: String =
        thread_rng().sample_iter(&Alphanumeric).take(5).collect();

    let mut verify_keys = BTreeMap::new();
    verify_keys.insert(format!("ed25519:{}", key_name), (pubkey, seckey));

    let keys = casniam::protocol::server_keys::make_server_keys(
        "example.com".to_string(),
        verify_keys,
        BTreeMap::new(),
    );

    HttpResponse::Ok().json(keys)
}

fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new().service(web::resource("/").to(render_server_keys))
    })
    .bind("127.0.0.1:8088")?
    .run()
}
