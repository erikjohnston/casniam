use futures::future::Future;
use petgraph::visit::Walker;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::pin::Pin;

use failure::Error;

use crate::state_map::StateMap;

pub mod events;
pub mod json;
pub mod server_keys;
pub mod v1;

pub trait Event: Clone + fmt::Debug {
    fn get_prev_event_ids(&self) -> Vec<&str>;
    fn get_event_id(&self) -> &str;
}

pub trait RoomState: Clone {
    type Event: Event;

    // TODO: Requires access to a store of some kind.
    fn resolve_state<'a>(
        states: Vec<&'a Self>,
        store: &'a impl EventStore,
    ) -> Pin<Box<Future<Output = Result<Self, Error>>>>;

    fn add_event<'a>(&mut self, event: &'a Self::Event);

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>>;

    // fn get_event(
    //     &self,
    //     store: &impl EventStore,
    //     etype: &str,
    //     state_key: &str,
    // ) -> Pin<Box<Future<Output = Result<Option<Self::Event>, Error>>>>;

    fn get_types(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Pin<Box<Future<Output = Result<StateMap<Self::Event>, Error>>>>;
}

pub trait AuthRules {
    type Event: Event;
    type State: RoomState;

    fn check(
        e: &Self::Event,
        s: &Self::State,
    ) -> Pin<Box<Future<Output = Result<(), Error>>>>;

    fn auth_types_for_event(event: &Self::Event) -> Vec<(String, String)>;
}

pub trait RoomVersion {
    type Event: Event;
    type State: RoomState<Event = Self::Event>;
    type Auth: AuthRules<Event = Self::Event, State = Self::State>;
}

pub trait EventStore {
    type Event: Event;

    fn missing_events<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        event_ids: I,
    ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>>;

    fn get_events(
        &self,
        event_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Pin<Box<Future<Output = Result<Vec<Self::Event>, Error>>>>;

    fn get_state_for<S: RoomState<Event = Self::Event>, T: AsRef<str>>(
        &self,
        event_ids: &[T],
    ) -> Pin<Box<Future<Output = Result<Option<S>, Error>>>>;
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
    ) -> Result<Vec<PersistEventInfo<V>>, Error> {
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

        let mut event_to_state: HashMap<String, V::State> = HashMap::new();

        for event_id in &missing {
            let state = await!(self.event_store.get_state_for(&[event_id]))?;
            event_to_state.insert(event_id.to_string(), state.unwrap());
        }

        let mut persisted_state: Vec<PersistEventInfo<V>> = Vec::new();
        for event in chunk.events {
            let event_id = event.get_event_id().to_string();

            let states = event
                .get_prev_event_ids()
                .iter()
                .map(|e| &event_to_state[e as &str])
                .collect();

            let state_before =
                await!(V::State::resolve_state(states, &self.event_store))?;

            // FIXME: Differentiate between DB and auth errors.
            let rejected =
                await!(V::Auth::check(&event, &state_before)).is_err();

            let mut state_after = state_before.clone();
            state_after.add_event(&event);

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
pub struct PersistEventInfo<R: RoomVersion> {
    pub event: R::Event,
    pub rejected: bool,
    pub state_before: R::State,
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
                .get_prev_event_ids()
                .iter()
                .map(|e| e.to_string())
                .collect(),
            forward_extremities: vec![event.get_event_id().to_string()]
                .into_iter()
                .collect(),
            event_ids: vec![event.get_event_id().to_string()]
                .into_iter()
                .collect(),
            events: vec![event],
        }
    }

    pub fn add_event(&mut self, event: E) -> Result<(), E> {
        let backwards: Vec<_> = event
            .get_prev_event_ids()
            .iter()
            .map(|e| e.to_string())
            .collect();

        if backwards.iter().all(|e| self.event_ids.contains(e)) {
            for e in &backwards {
                self.forward_extremities.remove(e);
            }
            self.forward_extremities
                .insert(event.get_event_id().to_string());
            self.event_ids.insert(event.get_event_id().to_string());
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
            .map(|e| (e.get_event_id().to_string(), e))
            .collect();

        for event in event_map.values() {
            for prev_event_id in &event.get_prev_event_ids() {
                if event_map.contains_key(prev_event_id as &str) {
                    graph.add_edge(event.get_event_id(), prev_event_id, 0);
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

    events.sort_unstable_by_key(|event| ordering[event.get_event_id()]);

    missing
}

fn get_missing<'a>(
    events: impl IntoIterator<Item = &'a (impl Event + 'a)>,
) -> HashSet<String> {
    let mut missing: HashSet<String> = HashSet::new();

    let event_map: HashMap<_, _> = events
        .into_iter()
        .map(|e| (e.get_event_id().to_string(), e))
        .collect();

    for event in event_map.values() {
        for prev_event_id in &event.get_prev_event_ids() {
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
    use futures::executor::block_on;
    use futures::future;

    #[derive(Deserialize, Debug, Clone)]
    struct TestEvent {
        event_id: String,
        prev_events: Vec<String>,
    }

    impl Event for TestEvent {
        fn get_prev_event_ids(&self) -> Vec<&str> {
            self.prev_events.iter().map(|e| &e as &str).collect()
        }

        fn get_event_id(&self) -> &str {
            &self.event_id
        }
    }

    #[test]
    fn test_dag_chunk() {
        let events = vec![
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
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
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
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
            },
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
            },
        ];

        let missing = topological_sort(&mut events);

        let order: Vec<&str> =
            events.iter().map(|e| e.get_event_id()).collect();

        let expected_order = vec!["B", "C", "D"];

        assert_eq!(order, expected_order);

        let expected_missing: HashSet<String> =
            vec!["A".to_string()].into_iter().collect();
        assert_eq!(missing, expected_missing);
    }

    #[derive(Clone)]
    struct DummyState;

    impl RoomState for DummyState {
        type Event = TestEvent;

        fn resolve_state<'a>(
            _states: Vec<&'a Self>,
            _store: &'a impl EventStore,
        ) -> Pin<Box<Future<Output = Result<Self, Error>>>> {
            Box::pin(future::ok(DummyState))
        }

        fn add_event<'a>(&mut self, _event: &'a Self::Event) {}

        // fn get_event(
        //     &self,
        //     store: &impl EventStore,
        //     etype: &str,
        //     state_key: &str,
        // ) -> Pin<Box<Future<Output = Result<Option<Self::Event>, Error>>>>
        // {
        //     unimplemented!()
        // }

        // fn get_event_id(
        //     &self,
        //     etype: &str,
        //     state_key: &str,
        // ) -> Pin<Box<Future<Output = Result<Option<String>, Error>>>> {
        //     unimplemented!()
        // }

        fn get_event_ids(
            &self,
            _types: impl IntoIterator<Item = (String, String)>,
        ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
            unimplemented!()
        }

        fn get_types(
            &self,
            _types: impl IntoIterator<Item = (String, String)>,
        ) -> Pin<Box<Future<Output = Result<StateMap<Self::Event>, Error>>>>
        {
            unimplemented!()
        }
    }

    struct DummyAuth;

    impl AuthRules for DummyAuth {
        type Event = TestEvent;
        type State = DummyState;

        fn check<'a>(
            _e: &Self::Event,
            _s: &Self::State,
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

    struct DummyStore;

    impl EventStore for DummyStore {
        type Event = TestEvent;

        fn missing_events<'a, I: IntoIterator<Item = &'a str>>(
            &self,
            _event_ids: I,
        ) -> Pin<Box<Future<Output = Result<Vec<String>, Error>>>> {
            unimplemented!()
        }

        fn get_events(
            &self,
            _event_ids: impl IntoIterator<Item = impl AsRef<str>>,
        ) -> Pin<Box<Future<Output = Result<Vec<TestEvent>, Error>>>> {
            unimplemented!()
        }

        fn get_state_for<S: RoomState, T: AsRef<str>>(
            &self,
            _event_ids: &[T],
        ) -> Pin<Box<Future<Output = Result<Option<S>, Error>>>> {
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
            },
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
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
