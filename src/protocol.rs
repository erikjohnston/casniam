use crate::stores::EventStore;

use futures::future::Future;
use log::info;
use petgraph::visit::Walker;
use serde::Serialize;
use serde_json::Value;
use sodiumoxide::crypto::sign;

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Debug;
use std::pin::Pin;

use failure::Error;

pub mod auth_rules;
pub mod client;
pub mod events;
pub mod json;
pub mod server_keys;
pub mod server_resolver;
pub mod state;

pub trait Event: Serialize + Sync + Send + Clone + fmt::Debug {
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

    fn from_builder<R: RoomVersion<Event = Self>, E: EventStore<Event = Self>>(
        builder: events::EventBuilder,
        event_store: &E,
    ) -> Pin<Box<dyn Future<Output = Result<Self, Error>>>>;

    fn sign(
        &mut self,
        server_name: String,
        key_name: String,
        key: &sign::SecretKey,
    );
}

pub trait DagNode {
    fn id(&self) -> &str;
    fn prevs(&self) -> Vec<&str>;
}

impl<E> DagNode for E
where
    E: Event,
{
    fn id(&self) -> &str {
        self.event_id()
    }

    fn prevs(&self) -> Vec<&str> {
        self.prev_event_ids()
    }
}

pub trait RoomStateResolver {
    type Auth: AuthRules;

    fn resolve_state<'a, S: RoomState>(
        states: Vec<S>,
        store: &'a impl EventStore<Event = <Self::Auth as AuthRules>::Event>,
    ) -> Pin<Box<dyn Future<Output = Result<S, Error>>>>;
}

pub trait RoomState: Clone + Debug + 'static {
    fn new() -> Self;

    fn add_event(&mut self, etype: String, state_key: String, event_id: String);

    fn remove(&mut self, etype: &str, state_key: &str);

    fn get(
        &self,
        event_type: impl Borrow<str>,
        state_key: impl Borrow<str>,
    ) -> Option<&str>;

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<String>;

    fn keys(&self) -> Vec<(&str, &str)>;
}

pub trait AuthRules {
    type Event: Event;

    fn check(
        e: &Self::Event,
        s: &impl RoomState,
        store: &impl EventStore<Event = Self::Event>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>>;

    fn auth_types_for_event(
        event_type: &str,
        state_key: Option<&str>,
        sender: &str,
        content: &serde_json::Map<String, Value>,
    ) -> Vec<(String, String)>;
}

pub trait RoomVersion: 'static {
    type Event: Event;
    type State: RoomStateResolver<Auth = Self::Auth>;
    type Auth: AuthRules<Event = Self::Event>;

    const VERSION: &'static str;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RoomVersion3;

impl RoomVersion for RoomVersion3 {
    type Event = events::v2::SignedEventV2;
    type Auth = auth_rules::AuthV1<events::v2::SignedEventV2>;
    type State = state::RoomStateResolverV2<
        auth_rules::AuthV1<events::v2::SignedEventV2>,
    >;

    const VERSION: &'static str = "3";
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RoomVersion4;

impl RoomVersion for RoomVersion4 {
    type Event = events::v3::SignedEventV3;
    type Auth = auth_rules::AuthV1<events::v3::SignedEventV3>;
    type State = state::RoomStateResolverV2<
        auth_rules::AuthV1<events::v3::SignedEventV3>,
    >;

    const VERSION: &'static str = "4";
}

pub trait FederationClient {
    fn get_missing_events<E: Event>(
        &self,
        forward: Vec<String>,
        back: Vec<String>,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<E>, Error>>>>;

    fn get_state_at<S: RoomState>(
        &self,
        event_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<S, Error>>>>;
}

pub trait FederationTransactionQueue {
    fn queue_events<V: RoomVersion>(
        &self,
        chunk: DagChunkFragment<V::Event>,
        destinations: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = ()>>>;
}

pub struct Handler<E: EventStore> {
    event_store: E,
    // client: Box<FederationClient>,
}

impl<ES: EventStore> Handler<ES> {
    pub fn new(event_store: ES) -> Self {
        Handler { event_store }
    }

    pub async fn handle_chunk<V: RoomVersion<Event = ES::Event> + 'static>(
        &self,
        chunk: DagChunkFragment<V::Event>,
    ) -> Result<Vec<PersistEventInfo<V, ES::RoomState>>, Error> {
        // TODO: This doesn't check signatures/hashes or whether it passes auth
        // checks based on auth events. Nor does it check for soft failures.
        // Former should be checked before we enter here, the latter after.

        let missing = get_missing(&chunk.events);

        if !missing.is_empty() {
            let unknown_events = self
                .event_store
                .missing_events(missing.iter().map(|e| e as &str))
                .await?;

            if !unknown_events.is_empty() {
                // TODO: Fetch missing events
                // TODO: Fetch state for gaps
                unimplemented!();
            }
        }

        let mut event_to_state_after: HashMap<String, ES::RoomState> =
            HashMap::new();

        for event_id in &missing {
            let state = self.event_store.get_state_for(&[event_id]).await?;
            event_to_state_after.insert(event_id.to_string(), state.unwrap());
        }

        let mut persisted_state = Vec::new();
        for event in chunk.events {
            let event_id = event.event_id().to_string();

            let states = event
                .prev_event_ids()
                .iter()
                .map(|e| event_to_state_after[e as &str].clone())
                .collect();

            let state_before: ES::RoomState =
                V::State::resolve_state(states, &self.event_store).await?;

            // FIXME: Differentiate between DB and auth errors.
            let rejected =
                match V::Auth::check(&event, &state_before, &self.event_store)
                    .await
                {
                    Ok(()) => false,
                    Err(err) => {
                        info!(
                            "Denied event {} because: {}",
                            event.event_type(),
                            err
                        );
                        true
                    }
                };

            let mut state_after = state_before.clone();
            if !rejected {
                if let Some(state_key) = event.state_key() {
                    state_after.add_event(
                        event.event_type().to_string(),
                        state_key.to_string(),
                        event.event_id().to_string(),
                    );
                }
            }

            persisted_state.push(PersistEventInfo {
                event,
                rejected,
                state_before,
            });

            event_to_state_after.insert(event_id, state_after);
        }

        Ok(persisted_state)
    }
}

#[derive(Debug, Clone)]
pub struct PersistEventInfo<R: RoomVersion, S: RoomState> {
    pub event: R::Event,
    pub rejected: bool,
    pub state_before: S,
}

#[derive(Debug, Default, Clone)]
pub struct DagChunkFragment<E> {
    event_ids: HashSet<String>,
    events: Vec<E>,
    backwards_extremities: HashSet<String>,
    forward_extremities: HashSet<String>,
}

impl<E> DagChunkFragment<E>
where
    E: DagNode,
{
    pub fn new() -> DagChunkFragment<E> {
        DagChunkFragment {
            event_ids: HashSet::new(),
            events: Vec::new(),
            backwards_extremities: HashSet::new(),
            forward_extremities: HashSet::new(),
        }
    }

    pub fn from_events(mut events: Vec<E>) -> Vec<DagChunkFragment<E>> {
        topological_sort(&mut events);

        let mut vec: Vec<DagChunkFragment<E>> = Vec::new();

        events.reverse();

        'outer: while let Some(mut event) = events.pop() {
            for chunk in &mut vec {
                event = match chunk.add_event(event) {
                    Ok(()) => continue 'outer,
                    Err(event) => event,
                }
            }

            vec.push(DagChunkFragment::from_event(event));
        }

        vec
    }

    pub fn from_event(event: E) -> DagChunkFragment<E> {
        DagChunkFragment {
            backwards_extremities: event
                .prevs()
                .iter()
                .map(|e| e.to_string())
                .collect(),
            forward_extremities: vec![event.id().to_string()]
                .into_iter()
                .collect(),
            event_ids: vec![event.id().to_string()].into_iter().collect(),
            events: vec![event],
        }
    }

    pub fn add_event(&mut self, event: E) -> Result<(), E> {
        let backwards: Vec<_> =
            event.prevs().iter().map(|e| e.to_string()).collect();

        if backwards.iter().all(|e| self.event_ids.contains(e)) {
            for e in &backwards {
                self.forward_extremities.remove(e);
            }
            self.forward_extremities.insert(event.id().to_string());
            self.event_ids.insert(event.id().to_string());
            self.events.push(event);
            return Ok(());
        }

        // TODO: Handle adding stuff to the start of the chunk
        // TODO: Handle adding batches of events

        Err(event)
    }

    pub fn forward_extremities(&self) -> &HashSet<String> {
        &self.forward_extremities
    }
}

/// Sorts the given vector of events into topological order, with "earliest"
/// events first.
fn topological_sort(events: &mut Vec<impl DagNode>) -> HashSet<String> {
    let (ordering, missing) = {
        let mut graph = petgraph::graphmap::DiGraphMap::new();
        let mut missing: HashSet<String> = HashSet::new();

        let event_map: HashMap<_, _> =
            events.iter().map(|e| (e.id().to_string(), e)).collect();

        for event in event_map.values() {
            for prev_event_id in &event.prevs() {
                if event_map.contains_key(prev_event_id as &str) {
                    graph.add_edge(event.id(), prev_event_id, 0);
                } else {
                    missing.insert(prev_event_id.to_string());
                }
            }
        }

        let reverse = petgraph::visit::Reversed(&graph);
        let walker = petgraph::visit::Topo::new(&reverse);

        let mut ordering = HashMap::new();
        for (idx, e) in walker.iter(&reverse).enumerate() {
            ordering.insert(e.to_string(), idx);
        }
        (ordering, missing)
    };

    events.sort_unstable_by_key(|event| ordering[event.id()]);

    missing
}

fn get_missing<'a>(
    events: impl IntoIterator<Item = &'a (impl Event + 'a)>,
) -> HashSet<String> {
    let mut missing: HashSet<String> = HashSet::new();

    let event_map: HashMap<_, _> = events
        .into_iter()
        .map(|e| (e.event_id().to_string(), e))
        .collect();

    for event in event_map.values() {
        for prev_event_id in &event.prev_event_ids() {
            if !event_map.contains_key(prev_event_id as &str) {
                missing.insert(prev_event_id.to_string());
            }
        }
    }

    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_map::StateMap;
    use futures::executor::block_on;
    use futures::future;

    #[derive(Serialize, Deserialize, Debug, Clone)]
    struct TestEvent {
        event_id: String,
        prev_events: Vec<String>,
        #[serde(rename = "type")]
        etype: String,
        state_key: Option<String>,
    }

    impl Event for TestEvent {
        fn prev_event_ids(&self) -> Vec<&str> {
            self.prev_events.iter().map(|e| &e as &str).collect()
        }

        fn event_id(&self) -> &str {
            &self.event_id
        }

        fn event_type(&self) -> &str {
            &self.etype
        }

        fn state_key(&self) -> Option<&str> {
            self.state_key.as_ref().map(|e| e as &str)
        }

        fn content(&self) -> &serde_json::Map<String, Value> {
            unimplemented!()
        }

        fn depth(&self) -> i64 {
            unimplemented!()
        }

        fn redacts(&self) -> Option<&str> {
            unimplemented!()
        }

        fn room_id(&self) -> &str {
            unimplemented!()
        }

        fn sender(&self) -> &str {
            unimplemented!()
        }

        fn auth_event_ids(&self) -> Vec<&str> {
            unimplemented!()
        }

        fn origin_server_ts(&self) -> u64 {
            unimplemented!()
        }

        fn from_builder<
            R: RoomVersion<Event = Self>,
            E: EventStore<Event = Self>,
        >(
            _builder: events::EventBuilder,
            _event_store: &E,
        ) -> Pin<Box<dyn Future<Output = Result<Self, Error>>>> {
            unimplemented!()
        }

        fn sign(
            &mut self,
            _server_name: String,
            _key_name: String,
            _key: &sign::SecretKey,
        ) {
            unimplemented!()
        }
    }

    #[test]
    fn test_dag_chunk() {
        let events = vec![
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
        ];

        let chunks = DagChunkFragment::from_events(events);

        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_get_missing() {
        let events = vec![
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
        ];

        let missing = get_missing(events.iter());

        let expected_missing: HashSet<String> =
            vec!["A".to_string()].into_iter().collect();
        assert_eq!(missing, expected_missing);
    }

    #[test]
    fn test_topo_sort() {
        let mut events = vec![
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
        ];

        let missing = topological_sort(&mut events);

        let order: Vec<&str> = events.iter().map(Event::event_id).collect();

        let expected_order = vec!["B", "C", "D"];

        assert_eq!(order, expected_order);

        let expected_missing: HashSet<String> =
            vec!["A".to_string()].into_iter().collect();
        assert_eq!(missing, expected_missing);
    }

    #[derive(Clone)]
    struct DummyState;

    impl RoomStateResolver for DummyState {
        type Auth = DummyAuth;

        fn resolve_state<'a, S: RoomState>(
            _states: Vec<S>,
            _store: &'a impl EventStore<Event = <Self::Auth as AuthRules>::Event>,
        ) -> Pin<Box<dyn Future<Output = Result<S, Error>>>> {
            Box::pin(future::ok(S::new()))
        }
    }

    struct DummyAuth;

    impl AuthRules for DummyAuth {
        type Event = TestEvent;

        fn check(
            _e: &Self::Event,
            _s: &impl RoomState,
            _store: &impl EventStore<Event = Self::Event>,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
            Box::pin(future::ok(()))
        }

        fn auth_types_for_event(
            _event_type: &str,
            _state_key: Option<&str>,
            _sender: &str,
            _content: &serde_json::Map<String, Value>,
        ) -> Vec<(String, String)> {
            unimplemented!()
        }
    }

    struct DummyVersion;

    impl RoomVersion for DummyVersion {
        type Event = TestEvent;
        type State = DummyState;
        type Auth = DummyAuth;

        const VERSION: &'static str = "DUMMY";
    }

    #[derive(Clone, Debug)]
    struct DummyStore;

    impl EventStore for DummyStore {
        type Event = TestEvent;
        type RoomState = StateMap<String>;
        type RoomVersion = DummyVersion;

        fn insert_events(
            &self,
            _events: impl IntoIterator<Item = (Self::Event, Self::RoomState)>,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
            unimplemented!()
        }

        fn missing_events<
            I: IntoIterator<Item = impl AsRef<str> + ToString>,
        >(
            &self,
            _event_ids: I,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>>>> {
            unimplemented!()
        }

        fn get_events(
            &self,
            _event_ids: impl IntoIterator<Item = impl AsRef<str>>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>>
        {
            unimplemented!()
        }

        fn get_state_for<T: AsRef<str>>(
            &self,
            _event_ids: &[T],
        ) -> Pin<Box<dyn Future<Output = Result<Option<Self::RoomState>, Error>>>>
        {
            unimplemented!()
        }

        fn get_conflicted_auth_chain(
            &self,
            _event_ids: Vec<Vec<impl AsRef<str>>>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Self::Event>, Error>>>>
        {
            unimplemented!()
        }
    }

    #[test]
    fn test_handle() {
        let handler = Handler {
            event_store: DummyStore,
        };

        let events = vec![
            TestEvent {
                event_id: "A".to_string(),
                prev_events: vec![],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
            },
        ];

        let mut chunks = DagChunkFragment::from_events(events);

        let fut = handler.handle_chunk::<DummyVersion>(chunks.pop().unwrap());

        let results = block_on(fut).unwrap();

        assert_eq!(results.len(), 4);

        for res in results {
            assert!(!res.rejected);
        }
    }
}
