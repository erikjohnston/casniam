use crate::json::signed::Signed;

pub mod v2;

#[derive(Deserialize)]
pub struct EventV2 {
    auth_events: Vec<String>,
    content: serde_json::Value,
    hashes: Vec<()>,
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

    pub fn hashes(&self) -> &[()] {
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


impl SignedEventV2 {

}
