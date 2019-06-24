pub mod v2;
pub mod v3;

use failure::Error;

use crate::protocol::{Event, RoomVersion};
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
        mut self,
        content: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        self.content = content;
        self
    }

    pub fn origin(mut self, origin: impl ToString) -> Self {
        self.origin = origin.to_string();
        self
    }

    pub fn origin_server_ts(mut self, origin_server_ts: u64) -> Self {
        self.origin_server_ts = origin_server_ts;
        self
    }

    pub fn with_prev_events(mut self, prev_events: Vec<String>) -> Self {
        self.prev_events = prev_events;
        self
    }

    pub async fn build<R: RoomVersion, E: EventStore<Event = R::Event>>(
        self,
        event_store: &E,
    ) -> Result<R::Event, Error> {
        let event = await!(R::Event::from_builder::<R, E>(self, event_store))?;
        Ok(event)
    }
}
