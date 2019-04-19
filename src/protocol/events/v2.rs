use crate::json::signed::Signed;

use crate::protocol::json::serialize_canonically_remove_fields;
use crate::protocol::{AuthRules, Event, EventStore, RoomState, RoomVersion};

use base64;
use failure::Error;
use sha2::{Digest, Sha256};
use std::cmp::max;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EventV2 {
    auth_events: Vec<String>,
    content: serde_json::Map<String, serde_json::Value>,
    depth: u64,
    hashes: EventHash,
    origin: String,
    origin_server_ts: u64,
    prev_events: Vec<String>,
    room_id: String,
    sender: String,
    #[serde(rename = "type")]
    event_type: String,
    state_key: Option<String>,
}

pub type SignedEventV2 = Signed<EventV2>;

impl EventV2 {
    pub async fn from_builder<
        R: RoomVersion<Event = EventV2>,
        E: EventStore,
    >(
        builder: super::EventBuilder,
        event_store: &E,
    ) -> Result<EventV2, Error> {
        let super::EventBuilder {
            event_type,
            state_key,
            sender,
            content,
            origin,
            origin_server_ts,
            room_id,
            prev_events,
        } = builder;

        let mut event = EventV2 {
            content,
            origin,
            origin_server_ts,
            room_id,
            sender,
            event_type,
            state_key,

            // We set the following attributes later
            auth_events: Vec::new(),
            depth: 0,
            hashes: EventHash::Sha256("".to_string()),
            prev_events: Vec::new(),
        };

        let auth_types = R::Auth::auth_types_for_event(&event);

        // TODO: Only pull out a subset of the state needed.
        let state =
            await!(event_store.get_state_for::<R::State, _>(&prev_events))?
                .ok_or_else(|| {
                    format_err!("No state for prev events: {:?}", &prev_events)
                })?;

        let auth_events = await!(state.get_event_ids(auth_types))?;

        let mut depth = 0;
        let evs: Vec<EventV2> = await!(event_store.get_events(&prev_events))?;
        for ev in evs {
            depth = max(ev.depth, depth);
        }

        event.depth = depth;
        event.auth_events = auth_events;
        event.prev_events = prev_events;

        let serialized =
            serialize_canonically_remove_fields(event.clone(), &["hashes"])?;

        let computed_hash = Sha256::digest(&serialized);

        event.hashes = EventHash::Sha256(base64::encode_config(
            &computed_hash,
            base64::STANDARD_NO_PAD,
        ));

        Ok(event)
    }

    pub fn auth_events(&self) -> &[String] {
        &self.auth_events
    }

    pub fn content(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.content
    }

    pub fn depth(&self) -> u64 {
        self.depth
    }

    pub fn hashes(&self) -> &EventHash {
        &self.hashes
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn origin_server_ts(&self) -> u64 {
        self.origin_server_ts
    }

    pub fn prev_events(&self) -> &[String] {
        &self.prev_events
    }

    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    pub fn sender(&self) -> &str {
        &self.sender
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn state_key(&self) -> Option<&str> {
        self.state_key.as_ref().map(|e| e as &str)
    }
}

impl Event for EventV2 {
    fn get_prev_event_ids(&self) -> Vec<&str> {
        self.prev_events().iter().map(|s| s as &str).collect()
    }
    fn get_event_id(&self) -> &str {
        "" // FIXME
    }
}

impl SignedEventV2 {}

pub fn redact(
    event: SignedEventV2,
) -> Result<SignedEventV2, serde_json::Error> {
    let etype = event.as_ref().event_type().to_string();
    let mut content = event.as_ref().content.clone();

    let val = serde_json::to_value(event)?;

    let allowed_keys = [
        "event_id",
        "sender",
        "room_id",
        "hashes",
        "signatures",
        "content",
        "type",
        "state_key",
        "depth",
        "prev_events",
        "prev_state",
        "auth_events",
        "origin",
        "origin_server_ts",
        "membership",
    ];

    let val = match val {
        serde_json::Value::Object(obj) => obj,
        _ => unreachable!(), // Events always serialize to an object
    };

    let mut val: serde_json::Map<_, _> = val
        .into_iter()
        .filter(|(k, _)| allowed_keys.contains(&(k as &str)))
        .collect();

    let mut new_content = serde_json::Map::new();

    let mut copy_content = |key: &str| {
        if let Some(v) = content.remove(key) {
            new_content.insert(key.to_string(), v);
        }
    };

    match &etype[..] {
        "m.room.membership" => copy_content("membership"),
        "m.room.create" => copy_content("creator"),
        "m.room.join_rule" => copy_content("join_rule"),
        "m.room.aliases" => copy_content("aliases"),
        "m.room.history_visibility" => copy_content("history_visibility"),
        "m.room.power_levels" => {
            for key in &[
                "ban",
                "events",
                "events_default",
                "kick",
                "redact",
                "state_default",
                "users",
                "users_default",
            ] {
                copy_content(key);
            }
        }
        _ => {}
    }

    val.insert(
        "content".to_string(),
        serde_json::Value::Object(new_content),
    );

    serde_json::from_value(serde_json::Value::Object(val))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum EventHash {
    #[serde(rename = "sha256")]
    Sha256(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::json::serialize_canonically_remove_fields;
    use base64;
    use sha2::{Digest, Sha256};

    #[test]
    fn test_deserialize() {
        let json = r#"{
            "auth_events": ["$VwZY3if3+/rKgEzJMyjVaDeQFs0xLzph6sGjHEzLn2E", "$VnU0XEK6VPvVIF2dYia3VEM4geCQ/crydE3oMUbkgzg", "$Rk6ptuAGr5qZt7qmRbHdn8OFjwWAcXqL9rUwH576pYE"],
            "content": {"body": "ok :)", "msgtype": "m.text"},
            "depth": 6555,
            "hashes": {"sha256": "of2ROvFl+BuX8KeRJV759pLPGQRdrL85a5NWnuRBBos"},
            "origin": "matrix.org",
            "origin_server_ts": 1554477158528,
            "prev_events": ["$YVjEKxL4rRhjLnTV4rXn8x+Df582SxEWzDwLsbZ8Za4"],
            "prev_state": [],
            "room_id": "!zVpPeWAObqutioiNzB:jki.re",
            "sender": "@dave:matrix.org",
            "type": "m.room.message",
            "signatures": {"matrix.org": {"ed25519:auto": "9wuGBfX5D1E8RZtO1OX5mqcqWZ9yJEUwlhyHyCZyGBc+ONiW/NwqrQAPVNcGfgjbbTYZZhgz6/gyUe4VdOGHCg"}},
            "unsigned": {"age_ts": 1554477158528}
        }"#;

        let event: SignedEventV2 = serde_json::from_str(json).unwrap();

        let hash = match event.as_ref().hashes() {
            EventHash::Sha256(s) => {
                base64::decode_config(&s, base64::STANDARD_NO_PAD).unwrap()
            }
        };

        let v = serialize_canonically_remove_fields(
            event,
            &["hashes", "signatures", "unsigned"],
        )
        .unwrap();
        let computed_hash = Sha256::digest(&v);
        assert_eq!(&hash[..], &computed_hash[..]);
    }

    #[test]
    fn test_redact() {
        let full_json = r#"{
            "auth_events": ["$VwZY3if3+/rKgEzJMyjVaDeQFs0xLzph6sGjHEzLn2E", "$VnU0XEK6VPvVIF2dYia3VEM4geCQ/crydE3oMUbkgzg", "$Rk6ptuAGr5qZt7qmRbHdn8OFjwWAcXqL9rUwH576pYE"],
            "content": {"body": "ok :)", "msgtype": "m.text"},
            "depth": 6555,
            "hashes": {"sha256": "of2ROvFl+BuX8KeRJV759pLPGQRdrL85a5NWnuRBBos"},
            "origin": "matrix.org",
            "origin_server_ts": 1554477158528,
            "prev_events": ["$YVjEKxL4rRhjLnTV4rXn8x+Df582SxEWzDwLsbZ8Za4"],
            "prev_state": [],
            "room_id": "!zVpPeWAObqutioiNzB:jki.re",
            "sender": "@dave:matrix.org",
            "type": "m.room.message",
            "signatures": {"matrix.org": {"ed25519:auto": "9wuGBfX5D1E8RZtO1OX5mqcqWZ9yJEUwlhyHyCZyGBc+ONiW/NwqrQAPVNcGfgjbbTYZZhgz6/gyUe4VdOGHCg"}},
            "unsigned": {"age_ts": 1554477158528}
        }"#;

        let redacted_json = r#"{
            "auth_events": ["$VwZY3if3+/rKgEzJMyjVaDeQFs0xLzph6sGjHEzLn2E", "$VnU0XEK6VPvVIF2dYia3VEM4geCQ/crydE3oMUbkgzg", "$Rk6ptuAGr5qZt7qmRbHdn8OFjwWAcXqL9rUwH576pYE"],
            "content": {},
            "depth": 6555,
            "hashes": {"sha256": "of2ROvFl+BuX8KeRJV759pLPGQRdrL85a5NWnuRBBos"},
            "origin": "matrix.org",
            "origin_server_ts": 1554477158528,
            "prev_events": ["$YVjEKxL4rRhjLnTV4rXn8x+Df582SxEWzDwLsbZ8Za4"],
            "prev_state": [],
            "room_id": "!zVpPeWAObqutioiNzB:jki.re",
            "sender": "@dave:matrix.org",
            "type": "m.room.message",
            "signatures": {"matrix.org": {"ed25519:auto": "9wuGBfX5D1E8RZtO1OX5mqcqWZ9yJEUwlhyHyCZyGBc+ONiW/NwqrQAPVNcGfgjbbTYZZhgz6/gyUe4VdOGHCg"}}
        }"#;

        let event: SignedEventV2 = serde_json::from_str(full_json).unwrap();

        let redacted = redact(event).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(redacted_json).unwrap(),
            serde_json::to_value(redacted).unwrap(),
        );
    }
}
