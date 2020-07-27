use casniam::protocol::client::HyperFederationClient;
use casniam::protocol::events::v2::{add_content_hash, SignedEventV2};
use casniam::protocol::Handler;
use casniam::protocol::{Event, RoomVersion, RoomVersion3};
use casniam::stores::memory::MemoryStoreFactory;

use serde_json::{json, Value};
use sodiumoxide::crypto::sign;

fn make_event(mut val: Value) -> SignedEventV2 {
    add_content_hash(&mut val).unwrap();
    serde_json::from_value(val).unwrap()
}

#[tokio::test]
async fn fetch_missing() {
    env_logger::builder().is_test(true).try_init().unwrap();

    let (_, secret_key) = sign::gen_keypair();
    let stores = MemoryStoreFactory::new();
    let client = HyperFederationClient::new(
        (),
        "server_name".to_string(),
        "key_name".to_string(),
        secret_key,
    );
    let handler = Handler::new(stores, client);

    let room_id = "!some_room:test";
    let creator = "@some_user:test";

    let _ = vec![
        json!({
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
        }),
        json!({
            "room_id": &room_id,
            "sender": &creator,
            "origin_server_ts": 0,
            "type": "m.room.member",
            "state_key": &creator,
            "content": {
                "membership": "join",
                "displayname": "Alice",
            },
        }),
        json!({
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
                "invite": 0
            },
        }),
        json!({
            "room_id": &room_id,
            "sender": &creator,
            "origin_server_ts": 0,
            "type": "m.room.join_rules",
            "state_key": "",
            "content": {
                "join_rule": "public",
            },
        }),
        json!({
            "room_id": room_id,
            "sender": creator,
            "origin_server_ts": 0,
            "type": "m.room.message",
            "content": {
                "msgtype": "m.text",
                "body": "Are you there?",
            },
        }),
    ];

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

    // let builders: Vec<<RoomVersion4 as RoomVersion>::Event> = event_json
    //     .into_iter()
    //     .map(|j| serde_json::from_value(j).expect("valid json"))
    //     .collect();

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![create_event, join_event],
        )
        .await
        .expect("handle events");
}
