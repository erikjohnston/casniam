#![feature(test)]

extern crate test;

use casniam::protocol::client::HyperFederationClient;
use casniam::protocol::events::v2::{add_content_hash, SignedEventV2};
use casniam::protocol::Handler;
use casniam::protocol::{Event, RoomVersion, RoomVersion3};
use casniam::stores::memory::MemoryStoreFactory;

use serde_json::{json, Value};
use sodiumoxide::crypto::sign;

use test::Bencher;

fn make_event(mut val: Value) -> SignedEventV2 {
    add_content_hash(&mut val).unwrap();
    serde_json::from_value(val).unwrap()
}

#[bench]
fn simple_send(b: &mut Bencher) {
    env_logger::builder().is_test(true).try_init().ok();

    let room_id = "!some_room:test";
    let creator = "@some_user:test";

    let create_event = make_event(json!({
        "room_id": &room_id,
        "sender": &creator,
        "origin_server_ts": 0,
        "type": "m.room.create",
        "state_key": "",
        "content": {
            "creator": &creator,
            "room_version": RoomVersion3::VERSION,
        },
        "auth_events": [],
        "prev_events": [],
        "depth": 1,
        "origin": "test",
    }));

    let join_event = make_event(json!({
        "room_id": &room_id,
        "sender": &creator,
        "origin_server_ts": 0,
        "type": "m.room.member",
        "state_key": &creator,
        "content": {
            "membership": "join",
            "displayname": "Alice",
        },
        "auth_events": [create_event.event_id()],
        "prev_events": [create_event.event_id()],
        "depth": 2,
        "origin": "test",
    }));

    let power_level_event = make_event(json!({
        "room_id": room_id,
        "sender": creator,
        "origin_server_ts": 0,
        "type": "m.room.power_levels",
        "state_key": "",
        "content": {
            "users": {
                creator: 100,
            },
            "users_default": 100,
            "events": {},
            "events_default": 0,
            "state_default": 50,
            "ban": 50,
            "kick": 50,
            "redact": 50,
            "invite": 0,
        },
        "auth_events": [create_event.event_id(), join_event.event_id()],
        "prev_events": [join_event.event_id()],
        "depth": 3,
        "origin": "test",
    }));

    let join_rule_event = make_event(json!({
        "room_id": &room_id,
        "sender": &creator,
        "origin_server_ts": 0,
        "type": "m.room.join_rules",
        "state_key": "",
        "content": {
            "join_rule": "public",
        },
        "auth_events": [create_event.event_id(), join_event.event_id(), power_level_event.event_id()],
        "prev_events": [power_level_event.event_id()],
        "depth": 4,
        "origin": "test",
    }));

    let message_event1 = make_event(json!({
        "room_id": room_id,
        "sender": creator,
        "origin_server_ts": 0,
        "type": "m.room.message",
        "content": {
            "msgtype": "m.text",
            "body": "Are you there?",
        },
        "auth_events": [create_event.event_id(), join_event.event_id(), power_level_event.event_id()],
        "prev_events": [join_rule_event.event_id()],
        "depth": 5,
        "origin": "test",
    }));

    let mut rt = tokio::runtime::Builder::new()
        .basic_scheduler()
        .build()
        .unwrap();

    let (_, secret_key) = sign::gen_keypair();
    let stores = MemoryStoreFactory::new();
    let client = HyperFederationClient::new(
        (),
        "server_name".to_string(),
        "key_name".to_string(),
        secret_key,
    );
    let handler = Handler::new(stores, client);

    let auth_events = vec![
        create_event.event_id().to_string(),
        join_event.event_id().to_string(),
        power_level_event.event_id().to_string(),
    ];

    let mut prev_message_id = message_event1.event_id().to_string();

    rt.block_on(async {
        handler
            .handle_new_timeline_events::<RoomVersion3>(
                "test",
                room_id,
                vec![
                    create_event,
                    join_event,
                    power_level_event,
                    join_rule_event,
                    message_event1,
                ],
            )
            .await
            .expect("handle events");
    });

    let mut depth = 5;

    b.iter(|| {
        rt.block_on(async {
            depth += 1;

            let new_message_event = make_event(json!({
                "room_id": room_id,
                "sender": creator,
                "origin_server_ts": 0,
                "type": "m.room.message",
                "content": {
                    "msgtype": "m.text",
                    "body": "Are you there?",
                },
                "auth_events": &auth_events,
                "prev_events": [&prev_message_id],
                "depth": depth,
                "origin": "test",
            }));

            prev_message_id = new_message_event.event_id().to_string();

            handler
                .handle_new_timeline_events::<RoomVersion3>(
                    "test",
                    room_id,
                    vec![new_message_event],
                )
                .await
                .expect("handle events");
        })
    });
}
