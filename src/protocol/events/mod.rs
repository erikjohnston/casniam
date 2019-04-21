pub mod v2;

use failure::Error;

use crate::protocol::{EventStore, RoomVersion};

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
        room_id: String,
        sender: String,
        event_type: String,
        state_key: Option<String>,
    ) -> Self {
        EventBuilder {
            origin: sender.clone(), // FIXME
            sender,
            event_type,
            state_key,
            room_id,
            prev_events: Vec::new(),
            content: serde_json::Map::new(),
            origin_server_ts: 0, // FIXME,
        }
    }

    pub fn with_content(
        &mut self,
        content: serde_json::Map<String, serde_json::Value>,
    ) -> &mut Self {
        self.content = content;
        self
    }

    pub fn origin(&mut self, origin: String) -> &mut Self {
        self.origin = origin;
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
        R: RoomVersion<Event = v2::EventV2>,
        E: EventStore<Event = v2::EventV2>,
    >(
        self,
        event_store: &E,
    ) -> Result<v2::EventV2, Error> {
        let res = await!(v2::EventV2::from_builder::<R, E>(self, event_store))?;
        Ok(res)
    }
}
