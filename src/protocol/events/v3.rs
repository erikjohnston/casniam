use crate::json::signed::Signed;

use crate::protocol::json::serialize_canonically_remove_fields;
use crate::protocol::{AuthRules, Event, RoomState, RoomVersion};
use crate::stores::EventStore;

use base64;
use failure::Error;
use futures::{Future, FutureExt};
use serde::de::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::cmp::max;
use std::pin::Pin;

use super::v2::{redact, EventV2};

#[derive(Debug, Clone, Serialize)]
pub struct SignedEventV3 {
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
    pub async fn from_builder<
        R: RoomVersion<Event = Self>,
        E: EventStore<Event = Self>,
    >(
        builder: super::EventBuilder,
        event_store: E,
    ) -> Result<Self, Error> {
        let auth_types = R::Auth::auth_types_for_event(
            &builder.event_type,
            builder.state_key.as_ref().map(|e| e as &str),
            &builder.sender,
            &builder.content,
        );

        // TODO: Only pull out a subset of the state needed.
        let state = await!(event_store.get_state_for(&builder.prev_events))?
            .ok_or_else(|| {
                format_err!(
                    "No state for prev events: {:?}",
                    &builder.prev_events
                )
            })?;

        let auth_events = state.get_event_ids(auth_types);

        let mut depth = 0;
        let evs = await!(event_store.get_events(&builder.prev_events))?;
        for ev in evs {
            depth = max(ev.depth(), depth);
        }

        let event = EventV2::from_builder(builder, auth_events, depth)?;
        let signed = Signed::wrap(event)?;
        Ok(Self::from_signed(signed))
    }

    pub fn from_signed(event: Signed<EventV2>) -> SignedEventV3 {
        let redacted: serde_json::Value =
            redact(&event).expect("EventV3 should always serialize.");

        let serialized = serialize_canonically_remove_fields(
            redacted,
            &["signatures", "unsigned"],
        )
        .expect("EventV3 should always serialize.");
        let computed_hash = Sha256::digest(&serialized);

        let event_id = format!(
            "${}",
            base64::encode_config(&computed_hash, base64::URL_SAFE_NO_PAD)
        );

        SignedEventV3 {
            event_id,
            signed: event,
        }
    }

    pub fn signed(&self) -> &Signed<EventV2> {
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

    fn from_builder<
        R: RoomVersion<Event = Self>,
        E: EventStore<Event = Self>,
    >(
        builder: super::EventBuilder,
        event_store: &E,
    ) -> Pin<Box<dyn Future<Output = Result<Self, Error>>>> {
        let event_store = event_store.clone();
        Self::from_builder::<R, E>(builder, event_store).boxed_local()
    }
}

impl<'de> Deserialize<'de> for SignedEventV3 {
    fn deserialize<D>(deserializer: D) -> Result<SignedEventV3, D::Error>
    where
        D: Deserializer<'de>,
    {
        let signed = Signed::<EventV2>::deserialize(deserializer)?;

        Ok(SignedEventV3::from_signed(signed))
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
            "auth_events": [
                "$nGmCFF6OvZhFWE18KW_FuEt5vc6LZZU3qtBedUPsdNU",
                "$RrGxF28UrHLmoASHndYb9Jb_1SFww2ptmtur9INS438",
                "$G56kaH7C6YswzrBt8jmNkukyn8k2gLiL1MvBUPuTRoU"
            ],
            "content": {
                "body": "Morning",
                "msgtype": "m.text"
            },
            "depth": 1977,
            "hashes": {
                "sha256": "dkIeXmX5z53VDYO7fKzXBs/rJxxmC57fNZ7RI/z6TIk"
            },
            "origin": "half-shot.uk",
            "origin_server_ts": 1560506336305,
            "prev_events": [
                "$SerKMzCN6PF7irH6u-xjyBcYer28yKQsfDAuErEB6lI"
            ],
            "prev_state": [],
            "room_id": "!uXDCzlYgCTHtiWCkEx:jki.re",
            "sender": "@Half-Shot:half-shot.uk",
            "type": "m.room.message",
            "signatures": {
                "half-shot.uk": {
                "ed25519:a_fBAF": "Ji64ZnlmzZvFh2avbXjslgztVpJq2R99GJcvAiIgS9e5js72Kb6MT9jG8ICwAJS0uvfe63y9EiT4ZISCexS4Dw"
                }
            },
            "unsigned": {
                "age_ts": 1560506336305
            }
        }"#;

        let event: SignedEventV3 = serde_json::from_str(json).unwrap();

        assert_eq!(
            event.event_id(),
            "$fiBKn85VUTUExDLt429XdiNizXtZl_2rjpfka1p3wQw"
        );

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
            "auth_events": [
                "$nGmCFF6OvZhFWE18KW_FuEt5vc6LZZU3qtBedUPsdNU",
                "$RrGxF28UrHLmoASHndYb9Jb_1SFww2ptmtur9INS438",
                "$G56kaH7C6YswzrBt8jmNkukyn8k2gLiL1MvBUPuTRoU"
            ],
            "content": {
                "body": "Morning",
                "msgtype": "m.text"
            },
            "depth": 1977,
            "hashes": {
                "sha256": "dkIeXmX5z53VDYO7fKzXBs/rJxxmC57fNZ7RI/z6TIk"
            },
            "origin": "half-shot.uk",
            "origin_server_ts": 1560506336305,
            "prev_events": [
                "$SerKMzCN6PF7irH6u-xjyBcYer28yKQsfDAuErEB6lI"
            ],
            "prev_state": [],
            "room_id": "!uXDCzlYgCTHtiWCkEx:jki.re",
            "sender": "@Half-Shot:half-shot.uk",
            "type": "m.room.message",
            "signatures": {
                "half-shot.uk": {
                "ed25519:a_fBAF": "Ji64ZnlmzZvFh2avbXjslgztVpJq2R99GJcvAiIgS9e5js72Kb6MT9jG8ICwAJS0uvfe63y9EiT4ZISCexS4Dw"
                }
            },
            "unsigned": {
                "age_ts": 1560506336305
            }
        }"#;

        let redacted_json = r#"{
            "auth_events": [
                "$nGmCFF6OvZhFWE18KW_FuEt5vc6LZZU3qtBedUPsdNU",
                "$RrGxF28UrHLmoASHndYb9Jb_1SFww2ptmtur9INS438",
                "$G56kaH7C6YswzrBt8jmNkukyn8k2gLiL1MvBUPuTRoU"
            ],
            "content": {},
            "depth": 1977,
            "hashes": {
                "sha256": "dkIeXmX5z53VDYO7fKzXBs/rJxxmC57fNZ7RI/z6TIk"
            },
            "origin": "half-shot.uk",
            "origin_server_ts": 1560506336305,
            "prev_events": [
                "$SerKMzCN6PF7irH6u-xjyBcYer28yKQsfDAuErEB6lI"
            ],
            "prev_state": [],
            "room_id": "!uXDCzlYgCTHtiWCkEx:jki.re",
            "sender": "@Half-Shot:half-shot.uk",
            "type": "m.room.message",
            "signatures": {
                "half-shot.uk": {
                "ed25519:a_fBAF": "Ji64ZnlmzZvFh2avbXjslgztVpJq2R99GJcvAiIgS9e5js72Kb6MT9jG8ICwAJS0uvfe63y9EiT4ZISCexS4Dw"
                }
            }
        }"#;

        let event: SignedEventV3 = serde_json::from_str(full_json).unwrap();

        let redacted: SignedEventV3 = redact(event.signed()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(redacted_json).unwrap(),
            serde_json::to_value(redacted).unwrap(),
        );
    }
}
