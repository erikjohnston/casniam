use casniam::protocol::client::{
    enc, GetMissingEventsResponse, GetStateIdsResponse, HyperFederationClient,
    MockRequester, Transaction,
};
use casniam::protocol::events::v2::{add_content_hash, SignedEventV2};
use casniam::protocol::Handler;
use casniam::protocol::{Event, RoomVersion, RoomVersion3};
use casniam::stores::memory::MemoryStoreFactory;

use futures::FutureExt;
use hyper::Response;
use serde_json::{json, Value};
use sodiumoxide::crypto::sign;

fn make_event(mut val: Value) -> SignedEventV2 {
    add_content_hash(&mut val).unwrap();
    serde_json::from_value(val).unwrap()
}

#[tokio::test]
async fn fetch_multiple_events() {
    env_logger::builder().is_test(true).try_init().ok();

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

    let message_event2 = make_event(json!({
        "room_id": room_id,
        "sender": creator,
        "origin_server_ts": 0,
        "type": "m.room.message",
        "content": {
            "msgtype": "m.text",
            "body": "Are you there?",
        },
        "auth_events": [create_event.event_id(), join_event.event_id(), power_level_event.event_id()],
        "prev_events": [message_event1.event_id()],
        "depth": 6,
        "origin": "test",
    }));

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![create_event, join_event, power_level_event, join_rule_event],
        )
        .await
        .expect("handle events");

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![message_event1, message_event2],
        )
        .await
        .expect("handle events");
}

#[tokio::test]
async fn fetch_missing() {
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

    let message_event2 = make_event(json!({
        "room_id": room_id,
        "sender": creator,
        "origin_server_ts": 0,
        "type": "m.room.message",
        "content": {
            "msgtype": "m.text",
            "body": "Are you there?",
        },
        "auth_events": [create_event.event_id(), join_event.event_id(), power_level_event.event_id()],
        "prev_events": [message_event1.event_id()],
        "depth": 6,
        "origin": "test",
    }));

    // We only give it message_event2, it requests 1.

    let mut requester = MockRequester::new();
    requester
        .expect_request()
        .withf(|request| {
            request.uri().host() == Some("test")
                && request.method() == "POST"
                && request
                    .uri()
                    .path()
                    .starts_with("/_matrix/federation/v1/get_missing_events/")
        })
        .return_once(move |_request| {
            let response = Response::builder()
                .status(200)
                .body(
                    serde_json::to_string(&GetMissingEventsResponse {
                        events: vec![message_event1],
                    })
                    .unwrap()
                    .into(),
                )
                .unwrap();

            futures::future::ok(response).boxed()
        });

    let (_, secret_key) = sign::gen_keypair();
    let stores = MemoryStoreFactory::new();
    let client = HyperFederationClient::new(
        requester,
        "server_name".to_string(),
        "key_name".to_string(),
        secret_key,
    );
    let handler = Handler::new(stores, client);

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![create_event, join_event, power_level_event, join_rule_event],
        )
        .await
        .expect("handle events");

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![message_event2],
        )
        .await
        .expect("handle events");
}

#[tokio::test]
async fn fetch_handle_gap() {
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

    let message_event2 = make_event(json!({
        "room_id": room_id,
        "sender": creator,
        "origin_server_ts": 0,
        "type": "m.room.message",
        "content": {
            "msgtype": "m.text",
            "body": "Are you there?",
        },
        "auth_events": [create_event.event_id(), join_event.event_id(), power_level_event.event_id()],
        "prev_events": [message_event1.event_id()],
        "depth": 6,
        "origin": "test",
    }));

    let message_event3 = make_event(json!({
        "room_id": room_id,
        "sender": creator,
        "origin_server_ts": 0,
        "type": "m.room.message",
        "content": {
            "msgtype": "m.text",
            "body": "Are you there?",
        },
        "auth_events": [create_event.event_id(), join_event.event_id(), power_level_event.event_id()],
        "prev_events": [message_event2.event_id()],
        "depth": 7,
        "origin": "test",
    }));

    // We only give it message_event3, it requests 2 and then state.

    let mut requester = MockRequester::new();
    requester
        .expect_request()
        .withf(|request| {
            request.uri().host() == Some("test")
                && request.method() == "POST"
                && request
                    .uri()
                    .path()
                    .starts_with("/_matrix/federation/v1/get_missing_events/")
        })
        .return_once(move |_request| {
            let response = Response::builder()
                .status(200)
                .body(
                    serde_json::to_string(&GetMissingEventsResponse {
                        events: vec![message_event2],
                    })
                    .unwrap()
                    .into(),
                )
                .unwrap();

            futures::future::ok(response).boxed()
        });

    let state_ids_response = GetStateIdsResponse {
        auth_chain_ids: vec![],
        pdu_ids: vec![
            create_event.event_id().to_string(),
            join_event.event_id().to_string(),
            power_level_event.event_id().to_string(),
        ],
    };

    requester
        .expect_request()
        .withf(|request| {
            request.uri().host() == Some("test")
                && request.method() == "GET"
                && request
                    .uri()
                    .path()
                    .starts_with("/_matrix/federation/v1/state_ids/")
        })
        .return_once(move |_request| {
            let response = Response::builder()
                .status(200)
                .body(
                    serde_json::to_string(&state_ids_response).unwrap().into(),
                )
                .unwrap();

            futures::future::ok(response).boxed()
        });

    let message_1_event_id = message_event1.event_id().to_string();

    requester
        .expect_request()
        .withf(move |request| {
            request.uri().host() == Some("test")
                && request.method() == "GET"
                && request.uri().path().starts_with(&format!(
                    "/_matrix/federation/v1/event/{}",
                    enc(&message_1_event_id)
                ))
        })
        .return_once(move |_request| {
            let response = Response::builder()
                .status(200)
                .body(
                    serde_json::to_string(&Transaction::<RoomVersion3> {
                        pdus: vec![message_event1],
                    })
                    .unwrap()
                    .into(),
                )
                .unwrap();

            futures::future::ok(response).boxed()
        });

    let (_, secret_key) = sign::gen_keypair();
    let stores = MemoryStoreFactory::new();
    let client = HyperFederationClient::new(
        requester,
        "server_name".to_string(),
        "key_name".to_string(),
        secret_key,
    );
    let handler = Handler::new(stores, client);

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![create_event, join_event, power_level_event, join_rule_event],
        )
        .await
        .expect("handle events");

    handler
        .handle_new_timeline_events::<RoomVersion3>(
            "test",
            room_id,
            vec![message_event3],
        )
        .await
        .expect("handle events");
}
