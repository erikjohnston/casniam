pub mod auth;
// pub mod state;

use serde_json::Value;

use crate::protocol::Event;

pub trait EventV1: Event {
    fn get_sender(&self) -> &str;
    fn get_room_id(&self) -> &str;
    fn get_content(&self) -> &serde_json::Map<String, Value>;
    fn get_type(&self) -> &str;
    fn get_state_key(&self) -> Option<&str>;
    fn get_redacts(&self) -> Option<&str>;
    fn get_depth(&self) -> i64;
}
