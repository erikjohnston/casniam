use futures::future::{Future, FutureExt};
use petgraph::visit::Walker;
use serde_json::Value;

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter;
use std::pin::Pin;

use failure::Error;

pub mod auth_rules;
pub mod events;
pub mod json;
pub mod server_keys;
pub mod state;
pub mod v1;

pub trait Event: Clone + fmt::Debug {
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
}

pub trait RoomStateResolver {
    type Auth: AuthRules;

    fn resolve_state<'a, S: RoomState>(
        states: Vec<S>,
        store: &'a impl EventStore<Event = <Self::Auth as AuthRules>::Event>,
    ) -> Pin<Box<Future<Output = Result<S, Error>>>>;
}

pub trait RoomState: Clone + 'static {
    fn new() -> Self;

    fn add_event<'a>(
        &mut self,
        etype: String,
        state_key: String,
        event_id: String,
    );

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
    ) -> Pin<Box<Future<Output = Result<(), Error>>>>;

    fn auth_types_for_event(event: &Self::Event) -> Vec<(String, String)>;
}

pub trait RoomVersion {
    type Event: Event;
    type State: RoomStateResolver<Auth = Self::Auth>;
    type Auth: AuthRules<Event = Self::Event>;
}

pub trait EventStore: Clone + 'static {
    type Event: Event;
    type RoomState: RoomState;
    type RoomVersion: RoomVersion<Event = Self::Event>;

    fn missing_events<'a, I: IntoIterator<Item = impl AsRef<str> + ToString>>(
        &self,
        event_ids: I,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>>;

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<Future<Output = Result<Vec<Self::Event>, Error>>>>;

    fn get_event(
        &self,
        event_id: impl AsRef<str>,
    ) -> Pin<Box<Future<Output = Result<Option<Self::Event>, Error>>>> {
        self.get_events(iter::once(event_id))
            .map(|r| r.map(|v| v.into_iter().next()))
            .boxed_local()
    }

    fn get_state_for<T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<Future<Output = Result<Option<Self::RoomState>, Error>>>>;

    fn get_conflicted_auth_chain(
        &self,
        event_ids: Vec<Vec<impl AsRef<str>>>,
    ) -> Pin<Box<Future<Output = Result<Vec<Self::Event>, Error>>>>;
}

pub trait FederationClient {
    fn get_missing_events<E: Event>(
        &self,
        forward: Vec<String>,
        back: Vec<String>,
        limit: usize,
    ) -> Pin<Box<Future<Output = Result<Vec<E>, Error>>>>;
    fn get_state_at<S: RoomState>(
        &self,
        event_id: &str,
    ) -> Pin<Box<Future<Output = Result<S, Error>>>>;
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
            let unknown_events = await!(self
                .event_store
                .missing_events(missing.iter().map(|e| e as &str)))?;

            if !unknown_events.is_empty() {
                // TODO: Fetch missing events
                // TODO: Fetch state for gaps
                unimplemented!();
            }
        }

        let mut event_to_state: HashMap<String, ES::RoomState> = HashMap::new();

        for event_id in &missing {
            let state = await!(self.event_store.get_state_for(&[event_id]))?;
            event_to_state.insert(event_id.to_string(), state.unwrap());
        }

        let mut persisted_state = Vec::new();
        for event in chunk.events {
            let event_id = event.event_id().to_string();

            let states = event
                .prev_event_ids()
                .iter()
                .map(|e| event_to_state[e as &str].clone())
                .collect();

            let state_before: ES::RoomState =
                await!(V::State::resolve_state(states, &self.event_store))?;

            // FIXME: Differentiate between DB and auth errors.
            let rejected = await!(V::Auth::check(
                &event,
                &state_before,
                &self.event_store
            ))
            .is_err();

            let mut state_after = state_before.clone();
            if let Some(state_key) = event.state_key() {
                state_after.add_event(
                    event.event_type().to_string(),
                    state_key.to_string(),
                    event.event_id().to_string(),
                );
            }

            persisted_state.push(PersistEventInfo {
                event,
                rejected,
                state_before,
            });

            event_to_state.insert(event_id, state_after);
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
    E: Event,
{
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
                .prev_event_ids()
                .iter()
                .map(|e| e.to_string())
                .collect(),
            forward_extremities: vec![event.event_id().to_string()]
                .into_iter()
                .collect(),
            event_ids: vec![event.event_id().to_string()].into_iter().collect(),
            events: vec![event],
        }
    }

    pub fn add_event(&mut self, event: E) -> Result<(), E> {
        let backwards: Vec<_> = event
            .prev_event_ids()
            .iter()
            .map(|e| e.to_string())
            .collect();

        if backwards.iter().all(|e| self.event_ids.contains(e)) {
            for e in &backwards {
                self.forward_extremities.remove(e);
            }
            self.forward_extremities
                .insert(event.event_id().to_string());
            self.event_ids.insert(event.event_id().to_string());
            self.events.push(event);
            return Ok(());
        }

        // TODO: Handle adding stuff to the start of the chunk
        // TODO: Handle adding batches of events

        Err(event)
    }
}

/// Sorts the given vector of events into topological order, with "earliest"
/// events first.
fn topological_sort(events: &mut Vec<impl Event>) -> HashSet<String> {
    let (ordering, missing) = {
        let mut graph = petgraph::graphmap::DiGraphMap::new();
        let mut missing: HashSet<String> = HashSet::new();

        let event_map: HashMap<_, _> = events
            .iter()
            .map(|e| (e.event_id().to_string(), e))
            .collect();

        for event in event_map.values() {
            for prev_event_id in &event.prev_event_ids() {
                if event_map.contains_key(prev_event_id as &str) {
                    graph.add_edge(event.event_id(), prev_event_id, 0);
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

    events.sort_unstable_by_key(|event| ordering[event.event_id()]);

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

    #[derive(Deserialize, Debug, Clone)]
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

        let order: Vec<&str> = events.iter().map(|e| e.event_id()).collect();

        let expected_order = vec!["B", "C", "D"];

        assert_eq!(order, expected_order);

        let expected_missing: HashSet<String> =
            vec!["A".to_string()].into_iter().collect();
        assert_eq!(missing, expected_missing);
    }

    #[derive(Clone)]
    struct DummyState;

    impl RoomStateResolver for DummyState {
        fn resolve_state<'a, S: RoomState>(
            _states: Vec<impl Borrow<S>>,
            _store: &'a impl EventStore,
        ) -> Pin<Box<Future<Output = Result<S, Error>>>> {
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
        ) -> Pin<Box<Future<Output = Result<(), Error>>>> {
            Box::pin(future::ok(()))
        }

        fn auth_types_for_event(_event: &Self::Event) -> Vec<(String, String)> {
            unimplemented!()
        }
    }

    struct DummyVersion;

    impl RoomVersion for DummyVersion {
        type Event = TestEvent;
        type State = DummyState;
        type Auth = DummyAuth;
    }

    #[derive(Clone, Debug)]
    struct DummyStore;

    impl EventStore for DummyStore {
        type Event = TestEvent;
        type RoomState = StateMap<String>;
        type RoomVersion = DummyVersion;

        fn missing_events<
            'a,
            I: IntoIterator<Item = impl AsRef<str> + ToString>,
        >(
            &self,
            _event_ids: I,
        ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
            unimplemented!()
        }

        fn get_events(
            &self,
            _event_ids: impl IntoIterator<Item = impl AsRef<str>>,
        ) -> Pin<Box<Future<Output = Result<Vec<Self::Event>, Error>>>>
        {
            unimplemented!()
        }

        fn get_state_for<T: AsRef<str>>(
            &self,
            _event_ids: &[T],
        ) -> Pin<Box<Future<Output = Result<Option<Self::RoomState>, Error>>>>
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
