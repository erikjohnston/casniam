pub mod v2;
pub mod v3;

use failure::Error;

use crate::protocol::RoomVersion;
use crate::stores::EventStore;

pub struct EventBuilder {
    event_type: String,
    state_key: Option<String>,
    sender: String,
    content: serde_json::Map<String, serde_json::Value>,
    origin: String,
    origin_server_ts: u64,
    room_id: String,
    prev_events: Vec<String>,
}

impl EventBuilder {
    pub fn new(
        room_id: impl ToString,
        sender: impl ToString,
        event_type: impl ToString,
        state_key: Option<impl ToString>,
    ) -> Self {
        EventBuilder {
            origin: sender.to_string(), // FIXME
            sender: sender.to_string(),
            event_type: event_type.to_string(),
            state_key: state_key.map(|s| s.to_string()),
            room_id: room_id.to_string(),
            prev_events: Vec::new(),
            content: serde_json::Map::new(),
            origin_server_ts: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    pub fn with_content(
        &mut self,
        content: serde_json::Map<String, serde_json::Value>,
    ) -> &mut Self {
        self.content = content;
        self
    }

    pub fn origin(&mut self, origin: impl ToString) -> &mut Self {
        self.origin = origin.to_string();
        self
    }

    pub fn origin_server_ts(&mut self, origin_server_ts: u64) -> &mut Self {
        self.origin_server_ts = origin_server_ts;
        self
    }

    pub fn with_prev_events(&mut self, prev_events: Vec<String>) -> &mut Self {
        self.prev_events = prev_events;
        self
    }

    pub async fn build_v2<
        R: RoomVersion<Event = v2::SignedEventV2>,
        E: EventStore<Event = v2::SignedEventV2>,
    >(
        self,
        event_store: &E,
    ) -> Result<v2::SignedEventV2, Error> {
        let res = await!(v2::EventV2::from_builder::<R, E>(self, event_store))?;
        Ok(res)
    }
}
