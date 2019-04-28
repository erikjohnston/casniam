#![feature(await_macro, async_await, futures_api)]

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate failure;

pub mod json;
pub mod protocol;
pub mod state_map;
pub mod stores;

use std::pin::Pin;

use failure::Error;
use futures::{future, Future, FutureExt};

use crate::protocol::{
    RoomState,
};
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

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
        future::ok(
            types
                .into_iter()
                .filter_map(|(t, s)| self.get(&t, &s))
                .cloned()
                .collect(),
        )
        .boxed()
    }

    fn get_types(
        &self,
        _types: impl IntoIterator<Item = (String, String)>,
    ) -> Pin<Box<Future<Output = Result<StateMap<String>, Error>>>> {
        // FIXME
        future::ok(self.clone()).boxed()
    }
}
