use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};

use std::marker::PhantomData;

use failure::Error;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use petgraph::{graphmap::DiGraphMap, Direction};
use serde_json::Value;

use crate::protocol::{
    AuthRules, Event, RoomState, RoomStateResolver, RoomVersion,
};
use crate::stores::EventStore;
use crate::StateMapWithData;

pub struct RoomStateResolverV2<R> {
    _room_version: PhantomData<R>,
}

impl<R> RoomStateResolver for RoomStateResolverV2<R>
where
    R: RoomVersion,
{
    type RoomVersion = R;

    fn resolve_state<S: RoomState<String>>(
        states: Vec<S>,
        store: &(impl EventStore<R> + Clone),
    ) -> BoxFuture<Result<S, Error>> {
        if states.len() == 1 {
            return futures::future::ok(states[0].clone()).boxed();
        }

        let store = store.clone();
        async move {
            let (
                unconflicted,
                mut conflicted_power_events,
                mut conflicted_standard_events,
                conflicted_auth_chain,
            ) = get_conflicted_set(&store, &states).await?;

            sort_by_reverse_topological_power_ordering(
                &mut conflicted_power_events,
                &conflicted_auth_chain,
                &store,
            )
            .await?;

            let resolved = iterative_auth_checks(
                &conflicted_power_events,
                &unconflicted,
                &store,
            )
            .await?;

            // FIXME: The unwrap_or_default here doesn't seem right.
            let power_level_id = resolved
                .get("m.room.power_levels", "")
                .map(|e| e as &str)
                .unwrap_or_default();

            mainline_ordering(
                &mut conflicted_standard_events,
                power_level_id,
                &store,
            )
            .await?;

            let resolved = iterative_auth_checks(
                &conflicted_standard_events,
                &resolved,
                &store,
            )
            .await?;
            Ok(resolved)
        }
        .boxed()
    }
}

/// Get all entries that do not match across all states
fn get_conflicted_events<S: RoomState<String>>(
    states: &[S],
) -> (S, BTreeMap<(String, String), BTreeSet<String>>) {
    let mut keys = BTreeSet::new();
    for state in states {
        keys.extend(state.keys());
    }

    let mut conflicted: BTreeMap<(String, String), BTreeSet<String>> =
        BTreeMap::new();
    let mut unconflicted = S::new();
    for (t, s) in keys {
        let mut curr_event_id = None;
        let mut is_key_conflicted = false;
        for state in states {
            if let Some(e) = state.get(t, s) {
                if !is_key_conflicted && curr_event_id.is_none() {
                    curr_event_id = Some(e);
                } else if is_key_conflicted || curr_event_id != Some(e) {
                    is_key_conflicted = true;
                    curr_event_id = None;

                    let key = (t.to_string(), s.to_string());
                    conflicted.entry(key).or_default().insert(e.to_string());
                }
            } else {
                is_key_conflicted = true;
                curr_event_id = None;

                let key = (t.to_string(), s.to_string());
                conflicted.entry(key).or_default();
            }
        }

        if let Some(e) = curr_event_id {
            unconflicted.add_event(t.to_string(), s.to_string(), e.to_string());
        }
    }

    (unconflicted, conflicted)
}

async fn get_conflicted_set<
    'a,
    R: RoomVersion,
    S: RoomState<String>,
    ST: EventStore<R>,
>(
    store: &'a ST,
    states: &'a [S],
) -> Result<(S, Vec<R::Event>, Vec<R::Event>, Vec<R::Event>), Error> {
    let (unconflicted, conflicted_keys) = get_conflicted_events(states);

    let mut state_sets = Vec::with_capacity(states.len());
    for state in states {
        let state_set: Vec<String> = state
            .borrow()
            .keys()
            .into_iter()
            .filter(|key| {
                conflicted_keys
                    .contains_key(&(key.0.to_string(), key.1.to_string()))
            })
            .filter_map(|key| state.borrow().get(key.0, key.1))
            .map(|e| e.to_string())
            .collect();

        state_sets.push(state_set);
    }

    let mut full_conflicted_set: BTreeMap<_, _> = store
        .get_conflicted_auth_chain(state_sets.clone())
        .await?
        .into_iter()
        .map(|e| (e.event_id().to_string(), e))
        .collect();

    let mut missing = Vec::new();
    for state_set in state_sets {
        for ev in state_set {
            if !full_conflicted_set.contains_key(&ev) {
                missing.push(ev);
            }
        }
    }

    let missing_evs = store
        .get_events(&missing.iter().map(|e| e as &str).collect::<Vec<_>>())
        .await?;
    full_conflicted_set.extend(
        missing_evs
            .into_iter()
            .map(|e| (e.event_id().to_string(), e)),
    );

    let mut conflicted_power_events = Vec::new();
    let mut conflicted_standard_events = BTreeMap::new();

    for ev in full_conflicted_set.values() {
        if is_power_event(ev) {
            conflicted_power_events.push(ev.clone());
        } else {
            conflicted_standard_events.insert(ev.event_id(), ev.clone());
        }
    }

    // We need to move all conflicted_standard_events that are in the power
    // events auth chain into conflicted_power_events
    // TODO: So much cloning...
    let mut stack: Vec<_> = conflicted_power_events.to_vec();
    while let Some(ev) = stack.pop() {
        if let Some(ee) = conflicted_standard_events.remove(ev.event_id()) {
            conflicted_power_events.push(ee);
        }

        for aid in ev.auth_event_ids() {
            if let Some(aev) = full_conflicted_set.get(aid) {
                stack.push(aev.clone());
            }
        }
    }

    Ok((
        unconflicted,
        conflicted_power_events,
        conflicted_standard_events
            .into_iter()
            .map(|(_, e)| e)
            .collect(),
        full_conflicted_set.into_iter().map(|(_, e)| e).collect(),
    ))
}

fn is_power_event(event: &impl Event) -> bool {
    match (event.event_type(), event.state_key()) {
        ("m.room.create", Some("")) => true,
        ("m.room.power_levels", Some("")) => true,
        ("m.room.join_rules", Some("")) => true,
        ("m.room.member", state_key) => {
            if let Some(Value::String(membership)) =
                event.content().get("membership")
            {
                match membership as &str {
                    "leave" | "ban" => Some(event.sender()) != state_key,
                    _ => false,
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

async fn sort_by_reverse_topological_power_ordering<
    'a,
    R: RoomVersion,
    ST: EventStore<R>,
>(
    events: &'a mut Vec<R::Event>,
    auth_diff: &'a [R::Event],
    store: &'a ST,
) -> Result<(), Error> {
    let auth_diff_map: BTreeMap<_, _> = auth_diff
        .iter()
        .map(|e| (e.event_id().to_string(), e))
        .collect();

    let mut graph = DiGraphMap::new();
    let mut ordering = BTreeMap::new();

    let mut to_add: Vec<_> = events.iter().map(Event::event_id).collect();
    while let Some(e_id) = to_add.pop() {
        let ev = auth_diff_map[e_id];

        for aid in ev.auth_event_ids() {
            if auth_diff_map.contains_key(aid) {
                // This needs to happen before we add an edge, as adding the
                // edge inserts the node.
                if !graph.contains_node(aid) {
                    to_add.push(aid);
                }

                graph.add_edge(e_id, aid, 0);
            }
        }
        let pl = get_power_level_for_sender(ev, store).await?;

        graph.add_node(e_id);
        ordering.insert(e_id, (-pl, ev.origin_server_ts(), e_id));
    }

    let mut graph = graph.into_graph::<u32>();

    let mut ordered = BTreeMap::new();
    let mut idx = 0;
    while let Some(node) = {
        graph.externals(Direction::Incoming).max_by_key(|e| {
            let ev_id = graph[*e];
            &ordering[ev_id]
        })
    } {
        let ev_id = graph[node];
        ordered.insert(ev_id.to_string(), idx);
        idx += 1;
        graph.remove_node(node);
    }

    events.sort_by_key(|e| -ordered[e.event_id()]);

    Ok(())
}

async fn get_power_level_for_sender<'a, R: RoomVersion, ST: EventStore<R>>(
    event: &'a R::Event,
    store: &'a ST,
) -> Result<i64, Error> {
    let auth_events = store.get_events(&event.auth_event_ids()).await?;

    let mut pl = None;
    let mut create = None;
    for aev in auth_events {
        match (aev.event_type(), aev.state_key()) {
            ("m.room.create", Some("")) => create = Some(aev),
            ("m.room.power_levels", Some("")) => pl = Some(aev),
            _ => {}
        }
    }

    let pl = if let Some(pl) = pl {
        pl
    } else {
        if let Some(create) = create {
            if create.sender() == event.sender() {
                return Ok(100);
            }
        }
        return Ok(0);
    };

    // FIXME: Handle non integer power levels? Ideally we'd have already parsed the contents structure.

    if let Some(Value::Number(num)) = pl
        .content()
        .get("users")
        .and_then(|m| m.get(event.sender()))
    {
        if let Some(u) = num.as_i64() {
            return Ok(u);
        } else {
            bail!(
                "power level for event {} was not in i64 range: {}",
                pl.event_id(),
                num
            );
        }
    }

    if let Some(Value::Number(num)) = pl.content().get("users_default") {
        if let Some(u) = num.as_i64() {
            return Ok(u);
        } else {
            bail!(
                "power level in event {} was not in i64 range: {}",
                pl.event_id(),
                num
            );
        }
    }

    Ok(0)
}

async fn iterative_auth_checks<
    'a,
    R: RoomVersion,
    S: RoomState<String>,
    ST: EventStore<R> + Clone,
>(
    sorted_events: &'a [R::Event],
    base_state: &'a S,
    store: &'a ST,
) -> Result<S, Error> {
    let mut new_state = base_state.clone();

    for event in sorted_events {
        let types = R::Auth::auth_types_for_event(
            event.event_type(),
            event.state_key(),
            event.sender(),
            event.content(),
        );

        let auth_events = store.get_events(&event.auth_event_ids()).await?;
        let mut auth_map = StateMapWithData::default();
        for e in auth_events {
            if let Some(state_key) = e.state_key() {
                if types.contains(&(
                    e.event_type().to_string(),
                    state_key.to_string(),
                )) {
                    auth_map.add_event(
                        e.event_type().to_string(),
                        state_key.to_string(),
                        e.clone(),
                    );
                }
            }
        }

        for (t, s) in types {
            if let Some(e) = new_state.get(t.as_str(), s.as_str()) {
                let event = store.get_event(e).await?;
                if let Some(event) = event {
                    auth_map.add_event(t.to_string(), s.to_string(), event);
                }
            }
        }

        let result = R::Auth::check(event, &auth_map);

        if result.is_ok() {
            new_state.add_event(
                event.event_type().to_string(),
                event
                    .state_key()
                    .expect("should be state event")
                    .to_string(),
                event.event_id().to_string(),
            );
        }
    }

    Ok(new_state)
}

async fn mainline_ordering<'a, R: RoomVersion, ST: EventStore<R>>(
    events: &'a mut Vec<R::Event>,
    resolved_power_id: &'a str,
    store: &'a ST,
) -> Result<(), Error> {
    let mut mainline = Vec::new();

    let mut p = resolved_power_id.to_string();
    'outer: loop {
        mainline.push(p.clone());
        if let Some(power_ev) = store.get_event(&p).await? {
            let auth_events =
                store.get_events(&power_ev.auth_event_ids()).await?;
            for auth_event in auth_events {
                if (auth_event.event_type(), auth_event.state_key())
                    == ("m.room.power_levels", Some(""))
                {
                    p = auth_event.event_id().to_string();
                    continue 'outer;
                }
            }
        }

        break;
    }

    let mut order_map = BTreeMap::new();
    for e in events.iter() {
        let depth = get_mainline_depth_for_event(e, &mainline, store).await?;
        order_map.insert(
            e.event_id().to_string(),
            (depth, e.origin_server_ts(), e.event_id().to_string()),
        );
    }

    events.sort_by_key(|e| &order_map[e.event_id()]);
    events.reverse();

    Ok(())
}

async fn get_mainline_depth_for_event<'a, R: RoomVersion, ST: EventStore<R>>(
    event: &'a R::Event,
    mainline: &'a [String],
    store: &'a ST,
) -> Result<usize, Error> {
    let mut curr_event = event.clone();

    'outer: loop {
        if let Some(pos) =
            mainline.iter().position(|e| e == curr_event.event_id())
        {
            return Ok(pos);
        }

        let auth_events =
            store.get_events(&curr_event.auth_event_ids()).await?;
        for auth_event in auth_events {
            if (auth_event.event_type(), auth_event.state_key())
                == ("m.room.power_levels", Some(""))
            {
                curr_event = auth_event;
                continue 'outer;
            }
        }

        return Ok(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::protocol::events::EventBuilder;
    use crate::protocol::{RoomStateResolver, RoomVersion, RoomVersion3};
    use crate::stores::memory::{new_memory_store, MemoryEventStore};
    use crate::stores::StateStore;
    use crate::StateMapWithData;

    use futures::executor::block_on;

    fn create_event(
        store: &MemoryEventStore<RoomVersion3, StateMapWithData<String>>,
        event_type: &str,
        state_key: Option<&str>,
        sender: &str,
        content: Option<serde_json::Value>,
        prev_events: Vec<String>,
    ) -> String {
        let mut builder =
            EventBuilder::new("fake_room_id", sender, event_type, state_key);

        if let Some(m) = content {
            builder = builder.with_content(m.as_object().unwrap().clone());
        }

        let mut state = block_on(store.get_state_after(
            &prev_events.iter().map(|e| e as &str).collect::<Vec<_>>(),
        ))
        .unwrap()
        .unwrap();

        builder = builder.with_prev_events(prev_events);

        let event = block_on(builder.build(store, store)).unwrap();

        if let Some(s) = state_key {
            state.insert(event_type, s, event.event_id().to_string());
        }

        let event_id = event.event_id().to_string();

        block_on(store.insert_state(&event, &mut state)).unwrap();
        block_on(store.insert_events(vec![event])).unwrap();

        event_id
    }

    #[test]
    fn test_get_conflicted_events() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            "@alice:test",
            None,
            vec![],
        );

        let state = block_on(store.get_state_after(&[&create]))
            .unwrap()
            .unwrap();

        let (unconflicted, _) = get_conflicted_events(&[state.clone()]);

        assert_eq!(unconflicted, state);
    }

    #[test]
    fn resolve_single() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            "@alice:test",
            None,
            vec![],
        );

        let state = block_on(store.get_state_after(&[&create]))
            .unwrap()
            .unwrap();

        let resolved =
            block_on(<RoomVersion3 as RoomVersion>::State::resolve_state(
                vec![state],
                &store,
            ))
            .unwrap();

        assert_eq!(resolved.map.len(), 1);
    }

    #[test]
    fn join_rule_evasion() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let alice = "@alice:test";
        let bob = "@bob:test";

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            alice,
            None,
            vec![],
        );

        let ima = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![create],
        );

        let ipower = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                },
            })),
            vec![ima],
        );

        let ijr = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "public",
            })),
            vec![ipower],
        );

        let imb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            bob,
            Some(json!({
                "membership": "join",
            })),
            vec![ijr.clone()],
        );

        let jr1 = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "private",
            })),
            vec![ijr],
        );

        let final_state = block_on(store.get_state_after(&[&imb, &jr1]))
            .unwrap()
            .unwrap();

        assert!(!final_state.map.contains_key("m.room.member", bob));
    }

    #[test]
    fn ban_vs_pl() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let alice = "@alice:test";
        let bob = "@bob:test";

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            alice,
            None,
            vec![],
        );

        let ima = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![create],
        );

        let ipower = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                },
            })),
            vec![ima],
        );

        let ijr = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "public",
            })),
            vec![ipower],
        );

        let imb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            bob,
            Some(json!({
                "membership": "join",
            })),
            vec![ijr],
        );

        let pa = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![imb],
        );

        let ma = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![pa.clone()],
        );

        let mb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            alice,
            Some(json!({
                "membership": "ban",
            })),
            vec![ma],
        );

        let pb = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            bob,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![pa.clone()],
        );

        let final_state = block_on(store.get_state_after(&[&mb, &pb]))
            .unwrap()
            .unwrap();

        assert_eq!(
            final_state.get("m.room.power_levels", ""),
            Some(&pa),
            "Testing power levels match"
        );
        assert_eq!(
            final_state.get("m.room.member", bob),
            Some(&mb),
            "Testing bobs membership match"
        );
    }

    #[test]
    fn offtopic_pl() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let alice = "@alice:test";
        let bob = "@bob:test";
        let charlie = "@charlie:test";

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            alice,
            None,
            vec![],
        );

        let ima = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![create],
        );

        let ipower = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                },
            })),
            vec![ima],
        );

        let ijr = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "public",
            })),
            vec![ipower],
        );

        let imb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            bob,
            Some(json!({
                "membership": "join",
            })),
            vec![ijr],
        );

        let imc = create_event(
            &store,
            "m.room.member",
            Some(charlie),
            charlie,
            Some(json!({
                "membership": "join",
            })),
            vec![imb],
        );

        let pa = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![imc],
        );

        let pb = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            bob,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                    charlie: 50,
                },
            })),
            vec![pa],
        );

        let pc = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            charlie,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                    charlie: 0,
                },
            })),
            vec![pb.clone()],
        );

        let final_state = block_on(store.get_state_after(&[&pb, &pc]))
            .unwrap()
            .unwrap();

        assert_eq!(
            final_state.get("m.room.power_levels", ""),
            Some(&pc),
            "Testing power levels match"
        );
    }

    #[test]
    fn topic_basic() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let alice = "@alice:test";
        let bob = "@bob:test";

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            alice,
            None,
            vec![],
        );

        let ima = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![create],
        );

        let ipower = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                },
            })),
            vec![ima],
        );

        let ijr = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "public",
            })),
            vec![ipower],
        );

        let imb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            bob,
            Some(json!({
                "membership": "join",
            })),
            vec![ijr],
        );

        let t1 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            alice,
            None,
            vec![imb],
        );

        let pa1 = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![t1],
        );

        let t2 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            alice,
            None,
            vec![pa1.clone()],
        );

        let pa2 = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 0,
                },
            })),
            vec![t2.clone()],
        );

        let pb = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            bob,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![pa1],
        );

        let t3 =
            create_event(&store, "m.room.topic", Some(""), bob, None, vec![pb]);

        let final_state = block_on(store.get_state_after(&[&pa2, &t3]))
            .unwrap()
            .unwrap();

        assert_eq!(
            final_state.get("m.room.power_levels", ""),
            Some(&pa2),
            "Testing power levels match"
        );

        assert_eq!(
            final_state.get("m.room.topic", ""),
            Some(&t2),
            "Testing topics match"
        );
    }

    #[test]
    fn topic_reset() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let alice = "@alice:test";
        let bob = "@bob:test";

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            alice,
            None,
            vec![],
        );

        let ima = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![create],
        );

        let ipower = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                },
            })),
            vec![ima],
        );

        let ijr = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "public",
            })),
            vec![ipower],
        );

        let imb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            bob,
            Some(json!({
                "membership": "join",
            })),
            vec![ijr],
        );

        let t1 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            alice,
            None,
            vec![imb],
        );

        let pa = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![t1.clone()],
        );

        let t2 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            bob,
            None,
            vec![pa.clone()],
        );

        let mb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            alice,
            Some(json!({
                "membership": "ban",
            })),
            vec![t2],
        );

        let final_state = block_on(store.get_state_after(&[&mb, &t1]))
            .unwrap()
            .unwrap();

        assert_eq!(
            final_state.get("m.room.power_levels", ""),
            Some(&pa),
            "Testing power levels match"
        );

        assert_eq!(
            final_state.get("m.room.topic", ""),
            Some(&t1),
            "Testing topics match"
        );

        assert_eq!(
            final_state.get("m.room.member", bob),
            Some(&mb),
            "Testing bobs membership match"
        );
    }

    #[test]
    fn topic() {
        let store: MemoryEventStore<RoomVersion3, _> = new_memory_store();

        let alice = "@alice:test";
        let bob = "@bob:test";

        let create = create_event(
            &store,
            "m.room.create",
            Some(""),
            alice,
            None,
            vec![],
        );

        let ima = create_event(
            &store,
            "m.room.member",
            Some(alice),
            alice,
            Some(json!({
                "membership": "join",
            })),
            vec![create],
        );

        let ipower = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                },
            })),
            vec![ima],
        );

        let ijr = create_event(
            &store,
            "m.room.join_rules",
            Some(""),
            alice,
            Some(json!({
                "join_rule": "public",
            })),
            vec![ipower],
        );

        let imb = create_event(
            &store,
            "m.room.member",
            Some(bob),
            bob,
            Some(json!({
                "membership": "join",
            })),
            vec![ijr],
        );

        let t1 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            alice,
            None,
            vec![imb],
        );

        let pa1 = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![t1],
        );

        let t2 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            alice,
            None,
            vec![pa1.clone()],
        );

        let pa2 = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            alice,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 0,
                },
            })),
            vec![t2],
        );

        let pb = create_event(
            &store,
            "m.room.power_levels",
            Some(""),
            bob,
            Some(json!({
                "users": {
                    alice: 100,
                    bob: 50,
                },
            })),
            vec![pa1],
        );

        let t3 =
            create_event(&store, "m.room.topic", Some(""), bob, None, vec![pb]);

        let msg = create_event(
            &store,
            "m.room.message",
            None,
            alice,
            None,
            vec![pa2.clone(), t3],
        );

        let t4 = create_event(
            &store,
            "m.room.topic",
            Some(""),
            alice,
            None,
            vec![msg.clone()],
        );

        let final_state = block_on(store.get_state_after(&[&msg, &t4]))
            .unwrap()
            .unwrap();

        assert_eq!(
            final_state.get("m.room.power_levels", ""),
            Some(&pa2),
            "Testing power levels match"
        );

        assert_eq!(
            final_state.get("m.room.topic", ""),
            Some(&t4),
            "Testing topics match"
        );
    }
}
