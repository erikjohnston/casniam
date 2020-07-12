use crate::stores::backed::BackedStore;
use crate::stores::{EventStore, StoreFactory};

use futures::future::BoxFuture;
use futures::future::Future;
use log::{info, warn};
use petgraph::visit::Walker;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sodiumoxide::crypto::sign;

use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Debug;
use std::iter::FromIterator;
use std::marker::PhantomData;
use std::pin::Pin;

use failure::Error;

pub mod auth_rules;
pub mod client;
pub mod events;
pub mod federation_api;
pub mod json;
pub mod server_keys;
pub mod server_resolver;
pub mod state;

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

    fn from_builder<R: RoomVersion<Event = Self>, S: RoomState>(
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

pub trait RoomStateResolver {
    type RoomVersion: RoomVersion;

    fn resolve_state<'a, S: RoomState>(
        states: Vec<S>,
        store: &'a (impl EventStore<Self::RoomVersion> + Clone),
    ) -> BoxFuture<Result<S, Error>>;
}

pub trait RoomState:
    IntoIterator<Item = ((String, String), String)>
    + FromIterator<((String, String), String)>
    + Default
    + Clone
    + Debug
    + Send
    + Sync
    + 'static
{
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
    type RoomVersion: RoomVersion;

    fn check<S: RoomState>(
        e: &<Self::RoomVersion as RoomVersion>::Event,
        s: &S,
        store: &(impl EventStore<Self::RoomVersion> + Clone),
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;

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

pub trait FederationClient {
    fn get_missing_events<E: Event>(
        &self,
        forward: Vec<String>,
        back: Vec<String>,
        limit: usize,
    ) -> BoxFuture<Result<Vec<E>, Error>>;

    fn get_state_at<S: RoomState>(
        &self,
        event_id: &str,
    ) -> BoxFuture<Result<S, Error>>;
}

pub trait FederationTransactionQueue {
    fn queue_events<V: RoomVersion>(
        &self,
        chunk: DagChunkFragment<V::Event>,
        destinations: Vec<String>,
    ) -> BoxFuture<()>;
}

#[derive(Clone)]
pub struct Handler<S: RoomState, F> {
    stores: F,
    client: Option<client::HyperFederationClient>,
    _data: PhantomData<S>, // client: Box<FederationClient>,
}

impl<S: RoomState, F: StoreFactory<S> + Clone + 'static> Handler<S, F> {
    pub fn new(stores: F, client: client::HyperFederationClient) -> Self {
        Handler {
            stores,
            client: Some(client),
            _data: PhantomData,
        }
    }

    pub fn no_http(stores: F) -> Self {
        Handler {
            stores,
            client: None,
            _data: PhantomData,
        }
    }

    pub async fn check_signatures_and_hashes<R: RoomVersion>(
        &self,
        _events: Vec<R::Event>,
    ) -> Result<(), Error> {
        // TODO:
        Ok(())
    }

    /// Check that the given events pass auth based on their auth events. Also fetches any missing
    /// auth events.
    pub async fn check_auth_auth_chain<R: RoomVersion>(
        &self,
        origin: &str,
        room_id: &str,
        mut events: Vec<R::Event>,
    ) -> Result<Vec<R::Event>, Error> {
        let stores = self.stores.clone();
        let event_store = stores.get_event_store::<R>();

        // Fetch missing auth events.
        while let Some(missing_events) = {
            let missing_events = event_store
                .missing_events(
                    &events.iter().map(|e| e.event_id()).collect::<Vec<_>>(),
                )
                .await?;

            if !missing_events.is_empty() {
                Some(missing_events)
            } else {
                None
            }
        } {
            let client = self
                .client
                .as_ref()
                .ok_or_else(|| format_err!("No http mode"))?;

            // Fetch missing events.
            for event_id in missing_events {
                let event =
                    client.get_event::<R>(origin, room_id, &event_id).await?;

                events.push(event);
            }
        }

        let auth_event_ids: Vec<&str> = events
            .iter()
            .flat_map(|event| event.auth_event_ids())
            .collect();

        let auth_events = event_store.get_events(&auth_event_ids).await?;

        let event_map: HashMap<&str, &R::Event> =
            auth_events.iter().map(|e| (e.event_id(), e)).collect();

        let backed_store: BackedStore<R, S, _> =
            BackedStore::new(event_store.clone());

        // We topological sort here to ensure that we handle events that are
        // referenced as auth events before subsequent events.
        topological_sort_by_func(&mut events, DagNode::auth_prev);

        let mut allowed_events = Vec::with_capacity(events.len());
        for event in events {
            let auth_state: S = event
                .auth_event_ids()
                .iter()
                .filter_map(|aid| event_map.get(aid))
                .filter_map(|e| {
                    e.state_key().map(|state_key| {
                        (
                            (e.event_type().to_string(), state_key.to_string()),
                            e.event_id().to_string(),
                        )
                    })
                })
                .collect();

            let rejected = match R::Auth::check(
                &event,
                &auth_state,
                &backed_store,
            )
            .await
            {
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
                backed_store.insert_event(event.clone()).await?;
                allowed_events.push(event);
            }
        }

        Ok(allowed_events)
    }

    /// Gets any missing events and returns a map of state for any new backwards
    /// extremities.
    pub async fn check_for_missing<R: RoomVersion>(
        &self,
        origin: &str,
        mut chunk: DagChunkFragment<R::Event>,
    ) -> Result<
        (
            DagChunkFragment<R::Event>,
            HashMap<String, client::GetStateIdsResponse>,
        ),
        Error,
    > {
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

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| format_err!("No http mode"))?;

        let room_store = stores.get_room_store::<R>();

        let set = room_store.get_forward_extremities(room.to_string()).await?;

        let current_extremities: Vec<_> =
            set.iter().map(|s| s as &str).collect();

        let new_extremities: Vec<&str> = chunk
            .forward_extremities()
            .iter()
            .map(|s| s as &str)
            .collect();

        let mut missing_events = client
            .get_missing_events::<R>(
                origin,
                &room,
                &current_extremities,
                &new_extremities,
            )
            .await?;

        topological_sort(&mut missing_events);

        for event in missing_events.into_iter().rev() {
            // TODO: We should probably not just throw away events that don't
            // fit
            chunk.add_event(event).ok();
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
            let mut new_events = Vec::new();

            for event_id in unknown_events {
                // TODO: Handle these calls failing.
                let state =
                    client.get_state_ids(origin, &room, &event_id).await?;

                let state_ids: Vec<&str> = state
                    .auth_chain_ids
                    .iter()
                    .chain(state.pdu_ids.iter())
                    .map(|e| e as &str)
                    .collect();

                let missing_events =
                    event_store.missing_events(&state_ids).await?;

                // Fetch missing events.
                for event_id in missing_events {
                    let event =
                        client.get_event::<R>(origin, &room, &event_id).await?;

                    new_events.push(event);
                }

                state_map.insert(event_id, state);
            }

            // We sort these so that if there are any dependencies we handle
            // them in the correct order.
            topological_sort_by_func(&mut new_events, DagNode::auth_prev);

            // TODO: Handle new_events
            assert!(new_events.is_empty());
        }

        Ok((chunk, state_map))
    }

    pub async fn handle_chunk<R: RoomVersion>(
        &self,
        chunk: DagChunkFragment<R::Event>,
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
            let unknown_events = event_store
                .missing_events(
                    &missing.iter().map(|e| e as &str).collect::<Vec<_>>(),
                )
                .await?;

            if !unknown_events.is_empty() {
                bail!("Unknown events: {:?}", unknown_events);
            }
        }

        let mut event_to_state_after: HashMap<String, S> = HashMap::new();

        for event_id in missing {
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

            let state_before: S =
                R::State::resolve_state(states, &store).await?;

            // FIXME: Differentiate between DB and auth errors.
            // TODO: Need to pass previous events to auth check
            let rejected =
                match R::Auth::check(&event, &state_before, &store).await {
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

            // state_store
            //     .insert_state(event.event_id(), state_before.clone())
            //     .await?;

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

#[derive(Debug, Clone)]
pub struct PersistEventInfo<R: RoomVersion, S: RoomState> {
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

            if !chunk_references_event {
                self.forward_extremities.insert(event.id().to_string());
            }

            self.event_ids.insert(event.id().to_string());
            self.events.push(event);
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
    use crate::state_map::StateMap;
    use crate::stores::memory;

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

        fn from_builder<R: RoomVersion<Event = Self>, S: RoomState>(
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
        type RoomVersion = DummyVersion;

        fn resolve_state<'a, S: RoomState>(
            _states: Vec<S>,
            _store: &'a (impl EventStore<Self::RoomVersion> + Clone),
        ) -> BoxFuture<Result<S, Error>> {
            Box::pin(future::ok(S::new()))
        }
    }

    struct DummyAuth;

    impl AuthRules for DummyAuth {
        type RoomVersion = DummyVersion;

        fn check<S: RoomState>(
            _e: &<Self::RoomVersion as RoomVersion>::Event,
            _s: &S,
            _store: &(impl EventStore<Self::RoomVersion> + Clone),
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
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
        let handler = Handler::<StateMap<String>, _>::no_http(
            memory::MemoryStoreFactory::new(),
        );

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
