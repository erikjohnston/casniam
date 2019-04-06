use crate::json::signed::Signed;

#[derive(Deserialize)]
pub struct EventV2 {
    auth_events: Vec<String>,
    content: serde_json::Value,
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
    pub fn auth_events(&self) -> &[String] {
        &self.auth_events
    }

    pub fn content(&self) -> &serde_json::Value {
        &self.content
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

impl SignedEventV2 {}

#[derive(Deserialize)]
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
}
