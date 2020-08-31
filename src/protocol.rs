use crate::state_map::StateMap;
use crate::stores::EventStore;

use futures::future::BoxFuture;
use futures::future::Future;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sodiumoxide::crypto::sign;

use std::borrow::Borrow;
use std::fmt;
use std::fmt::Debug;
use std::iter::FromIterator;
use std::{collections::BTreeSet, pin::Pin};

use failure::Error;

pub mod auth_rules;
pub mod client;
pub mod events;
pub mod federation_api;
pub mod handler;
pub mod json;
pub mod server_keys;
pub mod server_resolver;
pub mod state;

pub use handler::{DagChunkFragment, Handler, PersistEventInfo};

pub trait Event:
    DeserializeOwned + Serialize + Sync + Send + Clone + fmt::Debug
{
    fn auth_event_ids(&self) -> Vec<&str>;
    fn content(&self) -> &serde_json::Map<String, Value>;
    fn depth(&self) -> i64;
    fn event_id(&self) -> &str;
    fn event_type(&self) -> &str;
    fn origin_server_ts(&self) -> u64;
    fn prev_event_ids(&self) -> Vec<&str>;
    fn redacts(&self) -> Option<&str>;
    fn room_id(&self) -> &str;
    fn sender(&self) -> &str;
    fn state_key(&self) -> Option<&str>;

    fn from_builder<R: RoomVersion<Event = Self>, S: RoomState<String>>(
        builder: events::EventBuilder,
        state: S,
        prev_events: Vec<Self>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, Error>> + Send>>;

    fn sign(
        &mut self,
        server_name: String,
        key_name: String,
        key: &sign::SecretKey,
    );
}

impl<E> FromIterator<E> for StateMap<E>
where
    E: Event,
{
    fn from_iter<T: IntoIterator<Item = E>>(iter: T) -> StateMap<E> {
        let mut state_map = StateMap::new();

        for event in iter {
            let state_key = if let Some(state_key) = event.state_key() {
                state_key.to_string()
            } else {
                continue;
            };

            state_map.insert(
                &event.event_type().to_string(),
                &state_key,
                event,
            );
        }

        state_map
    }
}

pub trait RoomStateResolver {
    type RoomVersion: RoomVersion;

    fn resolve_state<'a, S: RoomState<String>>(
        states: Vec<S>,
        store: &'a (impl EventStore<Self::RoomVersion> + Clone),
    ) -> BoxFuture<Result<S, Error>>;
}

#[derive(Debug, Clone)]
pub enum StateMetadata {
    Persisted(usize),
    Delta {
        prev: usize,
        deltas: BTreeSet<(String, String)>,
    },
    New,
}

impl StateMetadata {
    pub fn changed(&mut self, etype: &str, state_key: &str) {
        match self {
            &mut StateMetadata::Persisted(sg) => {
                let mut deltas = BTreeSet::new();
                deltas.insert((etype.to_string(), state_key.to_string()));
                *self = StateMetadata::Delta { prev: sg, deltas };
            }
            StateMetadata::Delta { prev: _, deltas } => {
                deltas.insert((etype.to_string(), state_key.to_string()));
            }
            StateMetadata::New => {}
        }
    }

    pub fn mark_persisted(&mut self, sg: usize) {
        *self = StateMetadata::Persisted(sg);
    }
}

impl Default for StateMetadata {
    fn default() -> Self {
        StateMetadata::New
    }
}

pub trait RoomState<E>:
    IntoIterator<Item = ((String, String), E)>
    + FromIterator<((String, String), E)>
    + Default
    + Clone
    + Debug
    + Send
    + Sync
    + 'static
{
    fn new() -> Self;

    fn add_event(&mut self, etype: String, state_key: String, event_id: E);

    fn remove(&mut self, etype: &str, state_key: &str);

    fn get(
        &self,
        event_type: impl Borrow<str>,
        state_key: impl Borrow<str>,
    ) -> Option<&E>;

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<E>;

    fn keys(&self) -> Vec<(&str, &str)>;

    fn values<'a>(&'a self) -> Box<dyn Iterator<Item = &'a E> + Send + 'a>;

    fn iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = ((&'a str, &'a str), &'a E)> + Send + 'a>;

    fn metadata(&self) -> &StateMetadata;
    fn mark_persisted(&mut self, sg: usize);
}

pub trait AuthRules {
    type RoomVersion: RoomVersion;

    fn check<S: RoomState<<Self::RoomVersion as RoomVersion>::Event>>(
        e: &<Self::RoomVersion as RoomVersion>::Event,
        s: &S,
    ) -> Result<(), Error>;

    fn auth_types_for_event(
        event_type: &str,
        state_key: Option<&str>,
        sender: &str,
        content: &serde_json::Map<String, Value>,
    ) -> Vec<(String, String)>;
}

pub trait RoomVersion: Send + Sync + Clone + 'static {
    type Event: Event;
    type State: RoomStateResolver<RoomVersion = Self>;
    type Auth: AuthRules<RoomVersion = Self>;

    const VERSION: &'static str;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RoomVersion3;

impl RoomVersion for RoomVersion3 {
    type Event = events::v2::SignedEventV2;
    type Auth = auth_rules::AuthV1<Self>;
    type State = state::RoomStateResolverV2<Self>;

    const VERSION: &'static str = "3";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RoomVersion4;

impl RoomVersion for RoomVersion4 {
    type Event = events::v3::SignedEventV3;
    type Auth = auth_rules::AuthV1<Self>;
    type State = state::RoomStateResolverV2<Self>;

    const VERSION: &'static str = "4";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RoomVersion5;

impl RoomVersion for RoomVersion5 {
    type Event = events::v3::SignedEventV3;
    type Auth = auth_rules::AuthV1<Self>;
    type State = state::RoomStateResolverV2<Self>;

    const VERSION: &'static str = "5";
}
