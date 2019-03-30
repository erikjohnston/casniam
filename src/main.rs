#![feature(await_macro, async_await, futures_api)]

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate failure;

#[macro_use]
extern crate prettytable;
use prettytable::Table;

pub mod json;
pub mod protocol;
pub mod state_map;

use serde::de::IgnoredAny;

use std::pin::Pin;

use failure::Error;
use futures::executor::block_on;
use futures::{future, Future, FutureExt};

use crate::protocol::v1::{auth, EventV1};
use crate::protocol::{
    DagChunkFragment, Event, EventStore, Handler, RoomState, RoomVersion,
};
use crate::state_map::StateMap;

impl RoomState for StateMap<V1Event> {
    type Event = V1Event;

    fn resolve_state<'a>(
        states: Vec<&'a Self>,
        _store: &'a impl EventStore,
    ) -> Pin<Box<Future<Output = Result<Self, Error>>>> {
        let res = match states.len() {
            0 => Self::new(),
            1 => states[0].clone(),
            _ => unimplemented!(),
        };

        future::ok(res).boxed()
    }

    fn add_event<'a>(&mut self, event: &'a Self::Event) {
        if let Some(state_key) = event.get_state_key() {
            self.insert(event.get_type(), state_key, event.clone());
        }
    }

    fn get_types(
        &self,
        _types: impl IntoIterator<Item = (String, String)>,
    ) -> Pin<Box<Future<Output = Result<StateMap<Self::Event>, Error>>>> {
        future::ok(self.clone()).boxed()
    }
}

#[derive(Deserialize, Clone, Debug)]
struct V1Event {
    event_id: String,
    prev_events: Vec<(String, IgnoredAny)>,
    #[serde(rename = "type")]
    etype: String,
    state_key: Option<String>,
    room_id: String,
    content: serde_json::Map<String, serde_json::Value>,
    sender: String,
    redacts: Option<String>,
    depth: i64,
}

impl Event for V1Event {
    fn get_prev_event_ids(&self) -> Vec<&str> {
        self.prev_events.iter().map(|(e, _)| &e as &str).collect()
    }

    fn get_event_id(&self) -> &str {
        &self.event_id
    }
}

#[derive(Debug)]
struct RoomVersionV1;

impl RoomVersion for RoomVersionV1 {
    type Event = V1Event;
    type State = StateMap<V1Event>;
    type Auth = auth::AuthV1<Self::Event, Self::State>;
}

impl EventV1 for V1Event {
    fn get_sender(&self) -> &str {
        &self.sender
    }
    fn get_room_id(&self) -> &str {
        &self.room_id
    }
    fn get_content(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.content
    }
    fn get_type(&self) -> &str {
        &self.etype
    }
    fn get_state_key(&self) -> Option<&str> {
        self.state_key.as_ref().map(|s| s as &str)
    }
    fn get_redacts(&self) -> Option<&str> {
        self.redacts.as_ref().map(|s| s as &str)
    }
    fn get_depth(&self) -> i64 {
        self.depth
    }
}

struct DummyStore;

impl EventStore for DummyStore {
    fn missing_events<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        _event_ids: I,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
        unimplemented!()
    }

    fn get_events<E: Event>(
        &self,
        _event_ids: &[&str],
    ) -> Pin<Box<Future<Output = Result<Vec<E>, Error>>>> {
        unimplemented!()
    }

    fn get_state_for<S: RoomState>(
        &self,
        _event_ids: &[&str],
    ) -> Pin<Box<Future<Output = Result<Option<S>, Error>>>> {
        unimplemented!()
    }
}

pub fn main_old() {
    let evs = vec![
        r#"{"auth_events": [["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}], ["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}], ["$1551018411314oxMbu:jki.re", {"sha256": "j5OeKDtiq+j8FLdthUy61ULqEc6Efwgm/tHdGV+zUSc"}]], "prev_events": [["$1551018411314oxMbu:jki.re", {"sha256": "j5OeKDtiq+j8FLdthUy61ULqEc6Efwgm/tHdGV+zUSc"}]], "type": "m.room.join_rules", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"join_rule": "invite"}, "depth": 4, "prev_state": [], "state_key": "", "event_id": "$1551018411315nZGTA:jki.re", "origin": "jki.re", "origin_server_ts": 1551018411889, "hashes": {"sha256": "OVPGJh0Rs/LGJo1L/bYgmIqEQ69fHeqrzsRBlgvRZMk"}, "signatures": {"jki.re": {"ed25519:auto": "X0mHY60sqzDNCavu7wfvhB+up4oMNBq5RqiUYKHBwR/hq8fR8dAfr5fH8gfztVDEvoDWI+fRNtxH1c+/vwaNDQ"}}, "unsigned": {"age_ts": 1551018411889}}"#,
        r#"{"auth_events": [["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}], ["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}]], "prev_events": [["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}]], "type": "m.room.power_levels", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"users": {"@erikj:jki.re": 100}, "users_default": 0, "events": {"m.room.name": 50, "m.room.power_levels": 100, "m.room.history_visibility": 100, "m.room.canonical_alias": 50, "m.room.avatar": 50}, "events_default": 0, "state_default": 50, "ban": 50, "kick": 50, "redact": 50, "invite": 0}, "depth": 3, "prev_state": [], "state_key": "", "event_id": "$1551018411314oxMbu:jki.re", "origin": "jki.re", "origin_server_ts": 1551018411628, "hashes": {"sha256": "Ih9qMvmYK8gFjLk7U75Kl/yjuXEijxCJBXPeq06mqRY"}, "signatures": {"jki.re": {"ed25519:auto": "EpYgdscDn9BT3YX8ywjMCW/SS39xESfLqPUDSHnS4aUtND2KG7bdjQtriiQsqzfKUTTKyPRiLPdHcyyC96PbBA"}}, "unsigned": {"age_ts": 1551018411628}}"#,
        r#"{"auth_events": [["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}]], "prev_events": [["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}]], "type": "m.room.member", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"membership": "join", "displayname": "Erik", "avatar_url": "mxc://jki.re/GVSWoYAaZphVlOxPuwtQCFCl"}, "depth": 2, "prev_state": [], "state_key": "@erikj:jki.re", "event_id": "$1551018411313fejqG:jki.re", "origin": "jki.re", "origin_server_ts": 1551018411581, "hashes": {"sha256": "mpPy0t+R4a9B5SdAMNMAD5QAN9/9FoA71+11jMLhPYQ"}, "signatures": {"jki.re": {"ed25519:auto": "aQBAlxCQLe80XKcF+VNSlCJB6uQRfw38q7MJaHjnOzC/5OmK9a23BW/rzZSAEvJ7apwSMsITp2T0u7dTQTZKCg"}}, "unsigned": {"age_ts": 1551018411581}}"#,
        r#"{"auth_events": [["$1551018411314oxMbu:jki.re", {"sha256": "j5OeKDtiq+j8FLdthUy61ULqEc6Efwgm/tHdGV+zUSc"}], ["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}], ["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}]], "prev_events": [["$1551018411316KCByj:jki.re", {"sha256": "AIcGAVUwOtA5IvKI14F+PiRZn5hrw12VI989dQKvxpk"}]], "type": "m.room.guest_access", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"guest_access": "can_join"}, "depth": 6, "prev_state": [], "state_key": "", "event_id": "$1551018412317rSytZ:jki.re", "origin": "jki.re", "origin_server_ts": 1551018412030, "hashes": {"sha256": "ubkzmvO/urMHXhATw6KZzEKCB5bJAzqUIQv/KdtyqIg"}, "signatures": {"jki.re": {"ed25519:auto": "TXYdU7vj9JVbFtfjUd2FyoHXK7fH7pwngbe21zEo2aY4sSFlNMqz7oc9jvf4UBk6IR7s+PFICc9GuTc8JENFBQ"}}, "unsigned": {"age_ts": 1551018412030}}"#,
        r#"{"auth_events": [["$1551018411314oxMbu:jki.re", {"sha256": "j5OeKDtiq+j8FLdthUy61ULqEc6Efwgm/tHdGV+zUSc"}], ["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}], ["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}]], "prev_events": [["$1551018411315nZGTA:jki.re", {"sha256": "T3osafW3ovt4Kqm48qoAEDvKf5zJa3T+D5vTuQ56Um0"}]], "type": "m.room.history_visibility", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"history_visibility": "shared"}, "depth": 5, "prev_state": [], "state_key": "", "event_id": "$1551018411316KCByj:jki.re", "origin": "jki.re", "origin_server_ts": 1551018411955, "hashes": {"sha256": "VA7zFpM353XuguIWL7F0jcT7to20OWGo2VVr/FsXQdw"}, "signatures": {"jki.re": {"ed25519:auto": "oGQR/KOASzCRpy2/s+LILruYj2MKFgl4wcXuWNSZ8qWXH47HhVLDhZCD+ZxYU5iA7Ww9v/ylN8y7mec+KHGpBA"}}, "unsigned": {"age_ts": 1551018411955}}"#,
        r#"{"auth_events": [["$1551018411314oxMbu:jki.re", {"sha256": "j5OeKDtiq+j8FLdthUy61ULqEc6Efwgm/tHdGV+zUSc"}], ["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}], ["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}]], "prev_events": [["$1551018412317rSytZ:jki.re", {"sha256": "7XF3+7gwMMt79UG+TbOczmvP+x8glyPV7ChjpLVEOhA"}]], "type": "m.room.name", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"name": "TestRoom"}, "depth": 7, "prev_state": [], "state_key": "", "event_id": "$1551018412318VYNLu:jki.re", "origin": "jki.re", "origin_server_ts": 1551018412094, "hashes": {"sha256": "yzPwSvr4aUU0DvxaO3eKmoRhbrYri/tBdy39cKJTpdY"}, "signatures": {"jki.re": {"ed25519:auto": "7Tpy0vylre6eaFz0dR9kNcUT+GhU0l0rTw4lGPT87fDskDm9uIekQPr2csPDULS2i4v8wXfGH647hMXd2TAqAQ"}}, "unsigned": {"age_ts": 1551018412094}}"#,
        r#"{"auth_events": [["$1551018411314oxMbu:jki.re", {"sha256": "j5OeKDtiq+j8FLdthUy61ULqEc6Efwgm/tHdGV+zUSc"}], ["$1551018411312bOlzG:jki.re", {"sha256": "WwWVw1M3pyKlOWOj9OTeSR0dIXPFYttqAefJH5cr57g"}], ["$1551018411313fejqG:jki.re", {"sha256": "SoWbVs0mf21GfpgjXxyi8Es3qnfvUzNt7gK55/Mu2Yg"}]], "prev_events": [["$1551018412318VYNLu:jki.re", {"sha256": "vO7Jqx7PA6uOVlg/1TG3S66FHjfDo8hi55Wh/Km1eeE"}]], "type": "m.room.message", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"msgtype": "m.text", "body": "Test"}, "depth": 8, "prev_state": [], "event_id": "$1551018420319jhiHu:jki.re", "origin": "jki.re", "origin_server_ts": 1551018420517, "hashes": {"sha256": "cOWp33I3nSVrGVsSv5vI8ELkkVXTAITgjQyzglfZYFY"}, "signatures": {"jki.re": {"ed25519:auto": "P50exER1/gFRHcsrIJyE/+mJyYHR4+nwevmkrstTwESuMa8ielFMlGN6+g6QCP+6J8A8+gYQM4Ul+/AmUj5JAg"}}, "unsigned": {"age_ts": 1551018420517}}"#,
        r#"{"auth_events": [], "prev_events": [], "type": "m.room.create", "room_id": "!cjExvKHhAErxzYQmhV:jki.re", "sender": "@erikj:jki.re", "content": {"room_version": "1", "creator": "@erikj:jki.re"}, "depth": 1, "prev_state": [], "state_key": "", "event_id": "$1551018411312bOlzG:jki.re", "origin": "jki.re", "origin_server_ts": 1551018411528, "hashes": {"sha256": "dcUN13cqwwfx+p8GOHIO2uT8FrAplL5ALf+By5/oFUg"}, "signatures": {"jki.re": {"ed25519:auto": "aBKNgnuqhT10jnc8BeqJAukoNcDX+z/4eHPk/TlDvgV1UjoM6knLw+xagmJAPEQDbSjk+9a5qmzBbdXvu4sMAw"}}, "unsigned": {"age_ts": 1551018411528}}"#,
    ];

    let events: Vec<V1Event> = evs
        .into_iter()
        .map(|js| serde_json::from_str(js).unwrap())
        .collect();

    let chunks = DagChunkFragment::from_events(events);

    println!("Created {} chunk(s)", chunks.len());

    let handler = Handler::new(DummyStore);

    let mut last_state = None;
    for chunk in chunks {
        let results =
            block_on(handler.handle_chunk::<RoomVersionV1>(chunk)).unwrap();
        for res in &results {
            println!("{}: rejected={}", res.event.get_event_id(), res.rejected);

            last_state = Some(res.state_before.clone());
        }
    }

    if let Some(state) = last_state {
        println!("");

        let mut table = Table::new();
        table.set_format(
            *prettytable::format::consts::FORMAT_NO_BORDER_LINE_SEPARATOR,
        );

        table.set_titles(row!["Type", "State Key", "Event ID"]);

        for ((t, s), e) in state.iter() {
            table.add_row(row![t, s, e.get_event_id()]);
        }

        table.printstd();
    }
}

use actix_web::{server, App, HttpRequest, HttpResponse};
use sodiumoxide::crypto::sign;
use std::collections::BTreeMap;

fn render_server_keys(_req: &HttpRequest) -> HttpResponse {
    let (pubkey, seckey) = sign::gen_keypair();

    let mut verify_keys = BTreeMap::new();
    verify_keys.insert("ed25519:test".to_string(), (pubkey, seckey));

    let keys = protocol::server_keys::make_server_keys(
        "example.com".to_string(),
        verify_keys,
        BTreeMap::new(),
    );

    HttpResponse::Ok().json(keys)
}

fn main() {
    server::new(|| App::new().resource("/", |r| r.f(render_server_keys)))
        .bind("127.0.0.1:8088")
        .unwrap()
        .run();
}
