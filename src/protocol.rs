
use futures::future::FutureObj;
use petgraph::visit::Walker;

use std::collections::{HashMap, HashSet};

pub trait Event {
    fn get_prev_event_ids(&self) -> Vec<&str>;
    fn get_event_id(&self) -> &str;
}

pub trait RoomState: Clone {
    type Event: Event;

    // TODO: Requires access to a store of some kind.
    fn resolve_state<'a>(
        states: Vec<&'a Self>,
        store: &'a impl EventStore,
    ) -> Result<Self, ()>;
    fn add_event<'a>(&mut self, event: &'a Self::Event);
}

pub trait AuthRules {
    type Event: Event;
    type State: RoomState;

    fn check(e: &Self::Event, s: &Self::State) -> Result<(), ()>;
    fn get_auth_types<'a>(
        events: impl IntoIterator<Item = &'a (impl Event + 'a)>,
    ) -> Vec<(String, String)>;
}

pub trait RoomVersion<'a> {
    type Event: Event + 'a;
    type State: RoomState<Event = Self::Event> + 'a;
    type Auth: AuthRules<Event = Self::Event, State = Self::State> + 'a;
}

pub trait EventStore {
    fn missing_events<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        event_ids: I,
    ) -> FutureObj<Result<Vec<String>, ()>>;
    fn get_events<E: Event>(
        &self,
        event_ids: &[&str],
    ) -> FutureObj<Result<Vec<E>, ()>>;
    fn get_state_for<S: RoomState>(
        &self,
        event_ids: &[&str],
    ) -> FutureObj<Result<Option<S>, ()>>;
}

pub trait FederationClient {
    fn get_missing_events<E: Event>(
        &self,
        forward: Vec<String>,
        back: Vec<String>,
        limit: usize,
    ) -> FutureObj<Result<Vec<E>, ()>>;
    fn get_state_at<S: RoomState>(
        &self,
        event_id: &str,
    ) -> FutureObj<Result<S, ()>>;
}

pub struct Handler<E: EventStore> {
    event_store: E,
    // client: Box<FederationClient>,
}

impl<ES: EventStore> Handler<ES> {
    pub async fn handle_chunk<V: RoomVersion<'static>>(
        &self,
        chunk: DagChunkFragment<V::Event>,
    ) -> Result<Vec<PersistEventInfo>, ()> {
        // TODO: This doesn't check signatures/hashes or whether it passes auth
        // checks based on auth events. Nor does it check for soft failures.
        // Former should be checked before we enter here, the latter after.

        let missing = get_missing(&chunk.events);

        let unknown_events = await!(self
            .event_store
            .missing_events(missing.iter().map(|e| e as &str)))?;

        if !unknown_events.is_empty() {
            // TODO: Fetch missing events
            // TODO: Fetch state for gaps
            unimplemented!();
        }

        let mut event_to_state: HashMap<String, V::State> = HashMap::new();

        for event_id in &missing {
            let state = await!(self.event_store.get_state_for(&[event_id]))?;
            event_to_state.insert(event_id.to_string(), state.unwrap());
        }

        let mut persisted_state: Vec<PersistEventInfo> = Vec::new();
        for event in &chunk.events {
            let event_id = event.get_event_id();

            let states = event
                .get_prev_event_ids()
                .iter()
                .map(|e| &event_to_state[e as &str])
                .collect();

            let state_before =
                V::State::resolve_state(states, &self.event_store)?;

            let rejected = V::Auth::check(event, &state_before).is_err();

            persisted_state.push(PersistEventInfo {
                event_id: event_id.to_string(),
                rejected,
            });

            let mut state_after = state_before.clone();
            state_after.add_event(event);

            event_to_state.insert(event_id.to_string(), state_after);
        }

        Ok(persisted_state)
    }
}


#[derive(Debug, Clone)]
pub struct PersistEventInfo {
    event_id: String,
    rejected: bool,
    // state: RoomState,  // TODO: How do we talk about state changes?
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

fn topological_sort(events: &mut Vec<impl Event>) -> HashSet<String> {
    let (ordering, missing) = {
        let mut graph = petgraph::graphmap::DiGraphMap::new();
        let mut missing: HashSet<String> = HashSet::new();

        let event_map: HashMap<_, _> = events
            .iter()
            .map(|e| (e.get_event_id().to_string(), e))
            .collect();

        for (_, event) in &event_map {
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

    for (_, event) in &event_map {
        for prev_event_id in &event.get_prev_event_ids() {
            if !event_map.contains_key(prev_event_id as &str) {
                missing.insert(prev_event_id.to_string());
            }
        }
    }

    missing
}
