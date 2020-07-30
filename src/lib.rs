#![allow(clippy::type_complexity)]

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate failure;
#[cfg(test)]
#[macro_use]
extern crate serde_json;

#[track_caller]
macro_rules! expect_or_err {
    ($e:expr) => {
        if let Some(r) = $e {
            r
        } else {
            return Err(format_err!("'{}' was None", stringify!($e)).into());
        }
    };
}

pub mod json;
pub mod protocol;
pub mod state_map;
pub mod stores;

use std::borrow::Borrow;
use std::fmt::Debug;

use crate::protocol::RoomState;
use crate::state_map::StateMap;

impl<E> RoomState<E> for StateMap<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    fn new() -> Self {
        StateMap::new()
    }

    fn add_event(&mut self, etype: String, state_key: String, event_id: E) {
        self.insert(&etype, &state_key, event_id);
    }

    fn remove(&mut self, etype: &str, state_key: &str) {
        self.remove(etype, state_key);
    }

    fn get(
        &self,
        event_type: impl Borrow<str>,
        state_key: impl Borrow<str>,
    ) -> Option<&E> {
        self.get(event_type.borrow(), state_key.borrow())
    }

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<E> {
        types
            .into_iter()
            .filter_map(|(t, s)| self.get(&t, &s))
            .cloned()
            .collect()
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        self.keys().collect()
    }

    fn values<'a>(&'a self) -> Box<dyn Iterator<Item = &'a E> + 'a> {
        Box::new(self.values())
    }
}
