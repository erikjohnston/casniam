#![feature(await_macro, async_await)]

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate failure;

pub mod json;
pub mod protocol;
pub mod state_map;
pub mod stores;

use std::borrow::Borrow;

use crate::protocol::RoomState;
use crate::state_map::StateMap;

impl RoomState for StateMap<String> {
    fn new() -> Self {
        StateMap::new()
    }

    fn add_event<'a>(
        &mut self,
        etype: String,
        state_key: String,
        event_id: String,
    ) {
        self.insert(&etype, &state_key, event_id);
    }

    fn get(
        &self,
        event_type: impl Borrow<str>,
        state_key: impl Borrow<str>,
    ) -> Option<&str> {
        self.get(event_type.borrow(), state_key.borrow())
            .map(|e| e as &str)
    }

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<String> {
        types
            .into_iter()
            .filter_map(|(t, s)| self.get(&t, &s))
            .cloned()
            .collect()
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        self.keys().collect()
    }
}
