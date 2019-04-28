use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;

use failure::Error;
use petgraph::graphmap::DiGraphMap;
use serde_json::Value;

use crate::protocol::{Event, EventStore, RoomState, RoomStateResolver};

pub struct RoomStateResolverV2;

impl RoomStateResolver for RoomStateResolverV2 {
    fn resolve_state<'a, S: RoomState>(
        states: Vec<impl Borrow<S>>,
        store: &'a impl EventStore,
    ) -> Pin<Box<Future<Output = Result<S, Error>>>> {
        unimplemented!()
    }
}

/// Get all entries that do not match across all states
fn get_conflicted_events<S: RoomState>(
    states: &[&S],
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
                if is_key_conflicted || curr_event_id != Some(e) {
                    is_key_conflicted = true;

                    let key = (t.to_string(), s.to_string());
                    conflicted.entry(key).or_default().insert(e.to_string());
                } else if curr_event_id.is_none() {
                    curr_event_id = Some(e);
                }
            } else {
                is_key_conflicted = true;

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

async fn get_conflicted_set<'a, S: RoomState>(
    store: &'a impl EventStore,
    states: &'a [&S],
) -> Result<(), Error> {
    let (unconflicted, conflicted) = get_conflicted_events(states);

    let mut state_sets = Vec::with_capacity(states.len());
    for state in states {
        let state_set: Vec<_> = state
            .keys()
            .into_iter()
            .filter_map(|key| match key {
                ("m.room.create", "") => Some(key),
                ("m.room.power_levels", "") => Some(key),
                ("m.room.join_rules", "") => Some(key),
                ("m.room.member", _) => Some(key),
                ("m.room.third_party_invite", _) => Some(key),
                _ => None,
            })
            .filter_map(|key| state.get(key.0, key.1))
            .collect();

        state_sets.push(state_set);
    }

    let full_conflicted_set =
        await!(store.get_conflicted_auth_chain(state_sets))?;

    let mut conflicted_power_events = Vec::new();
    let mut conflicted_standard_events = Vec::new();

    for ev in full_conflicted_set {
        if is_power_event(&ev) {
            conflicted_power_events.push(ev);
        } else {
            conflicted_standard_events.push(ev);
        }
    }

    Ok(())
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

fn sort_by_reverse_topological_power_ordering(events: &mut Vec<impl Event>) {
    let mut graph = DiGraphMap::new();

    for ev in events {
        for aid in ev.auth_event_ids() {
            graph.add_edge(ev.event_id(), aid, 0);
        }
    }
}
