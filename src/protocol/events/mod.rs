pub mod v2;


// pub trait EventBuilder {
//     type EventType;

//     fn new(event_type: String, state_key: Option<String>, sender: String) -> Self;
//     fn with_content(&mut self, content: serde_json::Value) -> &mut Self;
//     fn origin(&mut self, origin: String) -> &mut Self;
//     fn origin_server_ts(&mut self, ts: u64) -> &mut Self;
//     fn room_id(&mut self, room_id: String) -> &mut Self;
//     fn build(self) -> Self::EventType;
// }

pub struct EventBuilder {
    event_type: String,
    state_key: Option<String>,
    sender: String,
    content: serde_json::Map<String, serde_json::Value>,
    origin: String,
    origin_server_ts: u64,
    room_id: String,
}

impl EventBuilder {
    pub fn new(room_id: String, sender: String, event_type: String, state_key: Option<String>) -> Self {
        EventBuilder {
            origin: sender.clone(),  // FIXME
            sender,
            event_type,
            state_key,
            room_id,
            content: serde_json::Map::new(),
            origin_server_ts: 0,  // FIXME,
        }
    }

    pub fn with_content(&mut self, content: serde_json::Map<String, serde_json::Value>) -> &mut Self {
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

    pub fn build_v2(self) -> v2::EventV2 {
        v2::EventV2::from_builder(self)
    }
}
