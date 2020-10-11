use crate::protocol::{
    client, AuthRules, Event, RoomState, RoomStateResolver, RoomVersion,
};
use crate::stores::backed::BackedStore;
use crate::stores::{EventStore, StoreFactory};
use crate::StateMapWithData;

use log::{info, warn};
use petgraph::visit::Walker;

use std::collections::{HashMap, HashSet};

use std::fmt::Debug;

use std::marker::PhantomData;

use failure::Error;

#[derive(Debug, Clone)]
pub struct Handler<S: RoomState<String>, F> {
    stores: F,
    client: client::HyperFederationClient,
    _data: PhantomData<S>,
}

impl<S: RoomState<String>, F: StoreFactory<S> + Clone + 'static> Handler<S, F> {
    pub fn new(stores: F, client: client::HyperFederationClient) -> Self {
        Handler {
            stores,
            client,
            _data: PhantomData,
        }
    }

    #[tracing::instrument]
    pub async fn handle_new_timeline_events<R: RoomVersion>(
        &self,
        origin: &str,
        room_id: &str,
        events: Vec<R::Event>,
    ) -> Result<Vec<PersistEventInfo<R, S>>, Error> {
        self.check_signatures_and_hashes::<R>(&events).await?;

        let allowed_events = self
            .check_auth_auth_chain_and_persist::<R>(origin, room_id, events)
            .await?;

        let chunks = DagChunkFragment::from_events(allowed_events);

        let mut persist_infos = Vec::new();
        for chunk in chunks {
            let (chunk, states) =
                self.check_for_missing::<R>(origin, room_id, chunk).await?;

            let mut persist_info = self.handle_chunk(chunk, states).await?;
            persist_infos.append(&mut persist_info);
        }

        Ok(persist_infos)
    }

    #[tracing::instrument]
    pub async fn check_signatures_and_hashes<R: RoomVersion>(
        &self,
        _events: &[R::Event],
    ) -> Result<(), Error> {
        // TODO:
        Ok(())
    }

    /// Check that the given events pass auth based on their auth events. Also fetches any missing
    /// auth events.
    #[tracing::instrument]
    pub async fn check_auth_auth_chain_and_persist<R: RoomVersion>(
        &self,
        origin: &str,
        room_id: &str,
        mut events: Vec<R::Event>,
    ) -> Result<Vec<R::Event>, Error> {
        let stores = self.stores.clone();
        let event_store = stores.get_event_store::<R>();

        // Fetch missing auth events.
        while let Some(missing_events) = {
            let extremities: Vec<_> = events
                .iter()
                .flat_map(|e| e.auth_event_ids())
                .filter(|e| !events.iter().any(|ev| ev.event_id() == *e))
                .collect();

            let missing_events =
                event_store.missing_events(&extremities).await?;

            if !missing_events.is_empty() {
                Some(missing_events)
            } else {
                None
            }
        } {
            // Fetch missing events.
            for event_id in missing_events {
                let event = self
                    .client
                    .get_event::<R>(origin, room_id, &event_id)
                    .await?;

                events.push(event);
            }
        }

        // We topological sort here to ensure that we handle events that are
        // referenced as auth events before subsequent events.
        // topological_sort_by_func(&mut events, DagNode::auth_prev);

        let mut allowed_events = Vec::with_capacity(events.len());
        for event in &events {
            // TODO: This will probably pull the same events out the DB each
            // loop, unless we've added some the previous iteration.

            let auth_events =
                event_store.get_events(&event.auth_event_ids()).await?;
            let event_map: HashMap<&str, &R::Event> = auth_events
                .iter()
                .chain(events.iter())
                .map(|e| (e.event_id(), e))
                .collect();

            let auth_state: StateMapWithData<R::Event> = event
                .auth_event_ids()
                .iter()
                .filter_map(|aid| event_map.get(aid))
                .filter_map(|e| {
                    e.state_key().map(|state_key| {
                        (
                            (e.event_type().to_string(), state_key.to_string()),
                            (*e).clone(),
                        )
                    })
                })
                .collect();

            let rejected = match R::Auth::check(&event, &auth_state) {
                Ok(()) => {
                    info!("Allowed (by auth chain) {}", event.event_id());
                    false
                }
                Err(err) => {
                    warn!(
                        "Denied event (by auth chain) {} because: {}",
                        event.event_type(),
                        err
                    );
                    true
                }
            };

            if !rejected {
                // XXX: We shouldn't need to clone here
                event_store.insert_event(event.clone()).await?;
                allowed_events.push(event.clone());
            } else {
                // TODO: What should we do here?
                todo!();
            }
        }

        Ok(allowed_events)
    }

    /// Gets any missing events and returns a map of state for any new backwards
    /// extremities.
    ///
    /// Returns the updated chunk and a map of the state after any missing
    /// events that are referenced.
    #[tracing::instrument]
    pub async fn check_for_missing<R: RoomVersion>(
        &self,
        origin: &str,
        room_id: &str,
        mut chunk: DagChunkFragment<R::Event>,
    ) -> Result<(DagChunkFragment<R::Event>, HashMap<String, S>), Error> {
        let stores = self.stores.clone();
        let event_store = stores.get_event_store::<R>();

        if chunk.backward_extremities().is_empty() {
            return Ok((chunk, HashMap::new()));
        }

        let unknown_events = event_store
            .missing_events(
                &chunk
                    .backward_extremities()
                    .iter()
                    .map(|e| e as &str)
                    .collect::<Vec<_>>(),
            )
            .await?;

        if unknown_events.is_empty() {
            return Ok((chunk, HashMap::new()));
        }

        info!("Unknown events: {:?}", unknown_events);

        let room = chunk.events[0].room_id().to_string();

        let room_store = stores.get_room_store::<R>();

        let set = room_store.get_forward_extremities(room.to_string()).await?;

        let current_extremities: Vec<_> =
            set.iter().map(|s| s as &str).collect();

        let new_extremities: Vec<&str> = chunk
            .forward_extremities()
            .iter()
            .map(|s| s as &str)
            .collect();

        let mut missing_events = self
            .client
            .get_missing_events::<R>(
                origin,
                &room,
                &current_extremities,
                &new_extremities,
            )
            .await?;

        topological_sort(&mut missing_events);

        info!(
            "Received events from missing events {:?}",
            missing_events
                .iter()
                .map(|e| e.event_id().to_string())
                .collect::<Vec<_>>()
        );

        // We need filter out invalid events now before we fetch the state.
        let missing_events = self
            .check_auth_auth_chain_and_persist::<R>(
                origin,
                room_id,
                missing_events,
            )
            .await?;

        for event in missing_events.into_iter().rev() {
            if let Err(_event) = chunk.add_event(event) {
                // TODO: We should probably not just throw away events that don't
                // fit
                todo!()
            }
        }

        let unknown_events = event_store
            .missing_events(
                &chunk
                    .backward_extremities()
                    .iter()
                    .map(|e| e as &str)
                    .collect::<Vec<_>>(),
            )
            .await?;

        let mut state_map = HashMap::new();
        if !unknown_events.is_empty() {
            info!(
                "Unknown events after fetching missing {:?}",
                unknown_events.iter().collect::<Vec<_>>()
            );

            for event_id in unknown_events {
                let mut new_events = Vec::new();

                match self.client.get_event::<R>(origin, &room, &event_id).await
                {
                    Ok(event) => new_events.push(event),
                    Err(e) => {
                        info!(
                            "Failed to get missing prev event, bailing: {}",
                            e
                        );
                        bail!(
                            "Failed to get missing prev event, bailing: {}",
                            e
                        );
                    }
                };

                // TODO: Handle these calls failing.
                let state_response =
                    self.client.get_state_ids(origin, &room, &event_id).await?;

                let state_ids: Vec<&str> = state_response
                    .auth_chain_ids
                    .iter()
                    .chain(state_response.pdu_ids.iter())
                    .map(|e| e as &str)
                    .collect();

                let missing_events =
                    event_store.missing_events(&state_ids).await?;

                // Fetch missing events.
                for state_event_id in missing_events {
                    match self
                        .client
                        .get_event::<R>(origin, &room, &state_event_id)
                        .await
                    {
                        Ok(event) => new_events.push(event),
                        Err(e) => {
                            info!("Failed to get missing event in state, bailing: {}", e);
                            bail!("Failed to get missing event in state, bailing: {}", e);
                        }
                    };
                }

                // We sort these so that if there are any dependencies we handle
                // them in the correct order.
                topological_sort_by_func(&mut new_events, DagNode::auth_prev);
                self.check_auth_auth_chain_and_persist::<R>(
                    origin, room_id, new_events,
                )
                .await?;

                // Figure out a way to not load all state events here.
                let new_event = event_store
                    .get_event(&event_id)
                    .await?
                    .ok_or_else(|| format_err!("State does not pass auth"))?;

                let state_events = event_store.get_events(&state_ids).await?;
                if state_events.len() != state_ids.len() {
                    bail!("State does not pass auth");
                }

                let mut state: S = state_events
                    .into_iter()
                    .filter_map(|e| {
                        let state_key = if let Some(state_key) = e.state_key() {
                            state_key.to_string()
                        } else {
                            return None;
                        };

                        Some((
                            (e.event_type().to_string(), state_key),
                            e.event_id().to_string(),
                        ))
                    })
                    .collect();

                if let Some(state_key) = new_event.state_key() {
                    state.add_event(
                        new_event.event_type().to_string(),
                        state_key.to_string(),
                        new_event.event_id().to_string(),
                    );
                }

                state_map.insert(event_id, state);
            }
        }

        // TODO: We need to check that any new state gets state res'd in to the
        // current state (to avoid the security hole).

        Ok((chunk, state_map))
    }

    #[tracing::instrument]
    pub async fn handle_chunk<R: RoomVersion>(
        &self,
        chunk: DagChunkFragment<R::Event>,
        mut event_to_state_after: HashMap<String, S>,
    ) -> Result<Vec<PersistEventInfo<R, S>>, Error> {
        // TODO: This doesn't check signatures/hashes or whether it passes auth
        // checks based on auth events. Nor does it check for soft failures.
        // Former should be checked before we enter here, the latter after.

        let stores = self.stores.clone();
        let event_store = stores.get_event_store::<R>();
        let state_store = stores.get_state_store::<R>();

        // let missing = get_missing(&chunk.events);
        let missing = &chunk.backward_extremities;

        if !missing.is_empty() {
            // TODO: need to check we have state for this
            let unknown_events = event_store
                .missing_events(
                    &missing.iter().map(|e| e as &str).collect::<Vec<_>>(),
                )
                .await?;

            if !unknown_events.is_empty() {
                bail!("Unknown events: {:?}", unknown_events);
            }
        }

        for event_id in missing {
            if event_to_state_after.contains_key(event_id) {
                continue;
            }

            let state = state_store.get_state_after(&[&event_id]).await?;
            let state = expect_or_err!(state);

            event_to_state_after.insert(event_id.to_string(), state);
        }

        let store: BackedStore<R, S, _> = BackedStore::new(event_store.clone());

        let mut persisted_state = Vec::new();
        for event in chunk.events {
            let event_id = event.event_id().to_string();

            let states = event
                .prev_event_ids()
                .iter()
                .map(|e| event_to_state_after[e as &str].clone())
                .collect();

            let mut state_before: S =
                R::State::resolve_state(states, &store).await?;

            let auth_types = R::Auth::auth_types_for_event(
                event.event_type(),
                event.state_key(),
                event.sender(),
                event.content(),
            );

            let state_event_ids: Vec<_> = auth_types
                .iter()
                .filter_map(|key| {
                    state_before.get(&key.0 as &str, &key.1 as &str)
                })
                .map(|s| s as &str)
                .collect();

            let auth_events = store.get_events(&state_event_ids).await?;
            let auth_state: StateMapWithData<R::Event> = auth_events
                .into_iter()
                .filter_map(|e| {
                    let state_key = if let Some(state_key) = e.state_key() {
                        state_key.to_string()
                    } else {
                        return None;
                    };

                    Some(((e.event_type().to_string(), state_key), e))
                })
                .collect();

            let rejected = match R::Auth::check(&event, &auth_state) {
                Ok(()) => {
                    info!("Allowed {}", event.event_id());
                    false
                }
                Err(err) => {
                    warn!(
                        "Denied event {} because: {}",
                        event.event_type(),
                        err
                    );
                    true
                }
            };

            state_store.insert_state(&event, &mut state_before).await?;

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

            store.insert_event(event.clone()).await?;

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

pub trait DagNode {
    fn id(&self) -> &str;
    fn prevs(&self) -> Vec<&str>;
    fn auth_prev(&self) -> Vec<&str>;
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

    fn auth_prev(&self) -> Vec<&str> {
        self.auth_event_ids()
    }
}

#[derive(Debug, Clone)]
pub struct PersistEventInfo<R: RoomVersion, S: RoomState<String>> {
    pub event: R::Event,
    pub rejected: bool,
    pub state_before: S,
}

/// A connected DAG fragment.
#[derive(Debug, Default, Clone)]
pub struct DagChunkFragment<E> {
    pub event_ids: HashSet<String>,
    pub events: Vec<E>,
    pub backward_extremities: HashSet<String>,
    pub forward_extremities: HashSet<String>,
}

impl<E> DagChunkFragment<E>
where
    E: DagNode,
{
    pub fn new() -> DagChunkFragment<E> {
        DagChunkFragment {
            event_ids: HashSet::new(),
            events: Vec::new(),
            backward_extremities: HashSet::new(),
            forward_extremities: HashSet::new(),
        }
    }

    pub fn into_events(self) -> Vec<E> {
        self.events
    }

    pub fn from_events(mut events: Vec<E>) -> Vec<DagChunkFragment<E>> {
        // TODO: This will make too many chunks and we should do a DFS(?)
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
            backward_extremities: event
                .prevs()
                .iter()
                .map(|&e| e.to_string())
                .collect(),
            forward_extremities: vec![event.id().to_string()]
                .into_iter()
                .collect(),
            event_ids: vec![event.id().to_string()].into_iter().collect(),
            events: vec![event],
        }
    }

    /// Add an event to the chunk. Returns Err if the event isn't connected to
    /// the chunk.
    pub fn add_event(&mut self, event: E) -> Result<(), E> {
        let backwards: Vec<_> =
            event.prevs().iter().map(|&e| e.to_string()).collect();

        let event_references_chunk =
            backwards.iter().any(|e| self.event_ids.contains(e));

        let chunk_references_event = self
            .events
            .iter()
            .flat_map(|e| e.prevs())
            .any(|e_id| e_id == event.id());

        if self.event_ids.is_empty()
            || event_references_chunk
            || chunk_references_event
        {
            for e in &backwards {
                self.forward_extremities.remove(e);
                if !self.event_ids.contains(e) {
                    self.backward_extremities.insert(e.to_string());
                }
            }

            self.backward_extremities.remove(event.id());

            if !chunk_references_event {
                self.forward_extremities.insert(event.id().to_string());
            }

            self.event_ids.insert(event.id().to_string());
            self.events.push(event);

            topological_sort(&mut self.events);

            return Ok(());
        }

        // TODO: Handle adding stuff to the start of the chunk
        // TODO: Handle adding batches of events

        Err(event)
    }

    /// Get the set of event IDs of the forward extremities.
    ///
    /// The event IDs returned are in the chunk.
    pub fn forward_extremities(&self) -> &HashSet<String> {
        &self.forward_extremities
    }

    /// Get the set of event IDs of the backwards extremities.
    ///
    /// The event IDs returned are *not* in the chunk.
    pub fn backward_extremities(&self) -> &HashSet<String> {
        &self.backward_extremities
    }
}

/// Sorts the given vector of events into topological order, with "earliest"
/// events first. Returns a list of event IDs referenced but not in the vec.
fn topological_sort(events: &mut Vec<impl DagNode>) -> HashSet<String> {
    topological_sort_by_func(events, DagNode::prevs)
}

/// Sorts the given vector of events into topological order, with "earliest"
/// events first. Returns a list of event IDs referenced but not in the vec.
fn topological_sort_by_func<N, F>(events: &mut Vec<N>, f: F) -> HashSet<String>
where
    N: DagNode,
    F: Fn(&N) -> Vec<&str>,
{
    let (ordering, missing) = {
        let mut graph = petgraph::graphmap::DiGraphMap::new();
        let mut missing: HashSet<String> = HashSet::new();

        let event_map: HashMap<_, _> =
            events.iter().map(|e| (e.id().to_string(), e)).collect();

        for event in event_map.values() {
            for prev_event_id in f(event) {
                if event_map.contains_key(prev_event_id as &str) {
                    graph.add_edge(event.id(), prev_event_id, 0);
                } else {
                    missing.insert(prev_event_id.to_string());
                }
            }
        }

        // TODO: Handle disconnected components.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::client::HyperFederationClient;
    use crate::protocol::events;
    use crate::stores::memory;
    use crate::StateMapWithData;

    use futures::executor::block_on;
    use futures::future;
    use futures::future::BoxFuture;
    use futures::Future;
    use serde_json::Value;
    use sodiumoxide::crypto::sign;
    use std::pin::Pin;

    #[derive(Serialize, Deserialize, Debug, Clone)]
    struct TestEvent {
        event_id: String,
        prev_events: Vec<String>,
        #[serde(rename = "type")]
        etype: String,
        state_key: Option<String>,
        content: serde_json::Map<String, Value>,
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
            &self.content
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
            "@foo:bar"
        }

        fn auth_event_ids(&self) -> Vec<&str> {
            vec![]
        }

        fn origin_server_ts(&self) -> u64 {
            unimplemented!()
        }

        fn from_builder<R: RoomVersion<Event = Self>, S: RoomState<String>>(
            _builder: events::EventBuilder,
            _state: S,
            _prev_events: Vec<Self>,
        ) -> Pin<Box<dyn Future<Output = Result<Self, Error>> + Send>> {
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
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
        ];

        let chunks = DagChunkFragment::from_events(events);

        assert_eq!(chunks.len(), 1);

        assert_eq!(
            chunks[0].forward_extremities(),
            &vec!["D".to_string()].into_iter().collect()
        );

        assert_eq!(
            chunks[0].backward_extremities(),
            &vec!["A".to_string()].into_iter().collect()
        )
    }

    #[test]
    fn test_topo_sort() {
        let mut events = vec![
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
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
        type RoomVersion = DummyVersion;

        fn resolve_state<S: RoomState<String>>(
            _states: Vec<S>,
            _store: &(impl EventStore<Self::RoomVersion> + Clone),
        ) -> BoxFuture<Result<S, Error>> {
            Box::pin(future::ok(S::new()))
        }
    }

    struct DummyAuth;

    impl AuthRules for DummyAuth {
        type RoomVersion = DummyVersion;

        fn check<S: RoomState<<Self::RoomVersion as RoomVersion>::Event>>(
            _e: &<Self::RoomVersion as RoomVersion>::Event,
            _s: &S,
        ) -> Result<(), Error> {
            Ok(())
        }

        fn auth_types_for_event(
            _event_type: &str,
            _state_key: Option<&str>,
            _sender: &str,
            _content: &serde_json::Map<String, Value>,
        ) -> Vec<(String, String)> {
            Vec::new()
        }
    }

    #[derive(Debug, Clone)]
    struct DummyVersion;

    impl RoomVersion for DummyVersion {
        type Event = TestEvent;
        type State = DummyState;
        type Auth = DummyAuth;

        const VERSION: &'static str = "DUMMY";
    }

    #[test]
    fn test_handle() {
        let (_pubkey, secret_key) = sign::gen_keypair();
        let client = HyperFederationClient::new(
            (),
            String::new(),
            String::new(),
            secret_key,
        );

        let handler = Handler::<StateMapWithData<String>, _>::new(
            memory::MemoryStoreFactory::new(),
            client,
        );

        let events = vec![
            TestEvent {
                event_id: "A".to_string(),
                prev_events: vec![],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "B".to_string(),
                prev_events: vec!["A".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "C".to_string(),
                prev_events: vec!["B".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
            TestEvent {
                event_id: "D".to_string(),
                prev_events: vec!["C".to_string()],
                etype: "type".to_string(),
                state_key: None,
                content: serde_json::Map::new(),
            },
        ];

        let fut = handler
            .handle_new_timeline_events::<DummyVersion>("foo", "bar", events);

        let results = block_on(fut).unwrap();

        assert_eq!(results.len(), 4);

        for res in results {
            assert!(!res.rejected);
        }
    }
}
