use crate::protocol::{Event, RoomState, RoomVersion};

use std::future::Future;
use std::pin::Pin;

use failure::Error;

use futures::FutureExt;

use std::iter;

pub mod memory;

pub trait EventStore: Clone + 'static {
    type Event: Event;
    type RoomState: RoomState;
    type RoomVersion: RoomVersion<Event = Self::Event>;

    fn insert_events(
        &self,
        events: impl IntoIterator<Item = (Self::Event, Self::RoomState)>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>>;

    fn missing_events<'a, I: IntoIterator<Item = impl AsRef<str> + ToString>>(
        &self,
        event_ids: I,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>>>>;

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>>;

    fn get_event(
        &self,
        event_id: impl AsRef<str>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Self::Event>, Error>>>> {
        self.get_events(iter::once(event_id))
            .map(|r| r.map(|v| v.into_iter().next()))
            .boxed_local()
    }

    fn get_state_for<T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<dyn Future<Output = Result<Option<Self::RoomState>, Error>>>>;

    fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<impl AsRef<str>>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>>;
}
