use crate::json::signed::Signed;

use crate::protocol::json::serialize_canonically_remove_fields;
use crate::protocol::{Event};

use base64;
use sha2::{Digest, Sha256};

use super::v2::{EventV2, redact};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedEventV3{
    #[serde(skip)]
    event_id: String,
    #[serde(flatten)]
    signed: Signed<EventV2>,
}

impl AsRef<EventV2> for SignedEventV3 {
    fn as_ref(&self) -> &EventV2 {
        self.signed.as_ref()
    }
}

impl SignedEventV3 {
    fn from_signed(event: Signed<EventV2>) -> SignedEventV3 {
        let redacted: EventV2 =
            redact(&event).expect("EventV2 should always serialize.");

        let serialized =
            serialize_canonically_remove_fields(redacted.clone(), &[])
                .expect("EventV2 should always serialize.");
        let computed_hash = Sha256::digest(&serialized);

        let event_id =
            base64::encode_config(&computed_hash, base64::URL_SAFE_NO_PAD);

        SignedEventV3 {
            event_id,
            signed: event,
        }
    }

    fn signed(&self) -> &Signed<EventV2> {
        &self.signed
    }
}

impl Event for SignedEventV3 {
    fn auth_event_ids(&self) -> Vec<&str> {
        self.signed
            .as_ref()
            .auth_events()
            .iter()
            .map(|s| s as &str)
            .collect()
    }

    fn content(&self) -> &serde_json::Map<String, serde_json::Value> {
        self.signed.as_ref().content()
    }

    fn depth(&self) -> i64 {
        self.signed.as_ref().depth()
    }

    fn event_id(&self) -> &str {
        &self.event_id
    }

    fn event_type(&self) -> &str {
        self.signed.as_ref().event_type()
    }

    fn origin_server_ts(&self) -> u64 {
        self.signed.as_ref().origin_server_ts()
    }

    fn prev_event_ids(&self) -> Vec<&str> {
        self.signed
            .as_ref()
            .prev_events()
            .iter()
            .map(|s| s as &str)
            .collect()
    }

    fn redacts(&self) -> Option<&str> {
        unimplemented!() // FIXME
    }

    fn room_id(&self) -> &str {
        self.signed.as_ref().room_id()
    }

    fn sender(&self) -> &str {
        self.signed.as_ref().sender()
    }

    fn state_key(&self) -> Option<&str> {
        self.signed.as_ref().state_key()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::events::v2::EventHash;
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

        let event: SignedEventV3 = serde_json::from_str(json).unwrap();

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

        let event: SignedEventV3 = serde_json::from_str(full_json).unwrap();

        let redacted: SignedEventV3 = redact(event.signed()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(redacted_json).unwrap(),
            serde_json::to_value(redacted).unwrap(),
        );
    }
}
