use std::collections::{HashMap, HashSet};

use sha1::Sha1;
use smallvec::SmallVec;

use super::auth;
use super::EventV1;
use crate::state_map::{StateMap, WellKnownEmptyKeys};

#[derive(Debug, Clone, Default)]
pub struct ResolvedState {
    // The full resolved state
    pub state: StateMap<String>,
    /// State keys -> senders
    pub conflicted_senders: StateMap<HashSet<String>>,
    pub delta_set: usize,
    pub delta: StateMap<String>,
}

/// Resolves a list of states to a single state.
pub fn resolve_state<E>(
    state_sets: Vec<&StateMap<String>>,
    event_map: &HashMap<String, E>,
) -> ResolvedState
where
    E: EventV1,
{
    if state_sets.len() == 0 {
        return ResolvedState::default();
    }

    if state_sets.len() == 1 {
        return ResolvedState {
            state: state_sets[0].clone(),
            conflicted_senders: StateMap::new(),
            delta_set: 0,
            delta: StateMap::new(),
        };
    }

    let mut unconflicted = state_sets[0].clone();
    let mut conflicted: StateMap<SmallVec<[&E; 5]>> = StateMap::new();

    for map in &state_sets[1..] {
        'outer: for ((t, s), eid) in map.iter() {
            if let Some(mut v) = conflicted.get_mut(t, s) {
                for ev in v.iter() {
                    if &ev.get_event_id() == eid {
                        continue 'outer;
                    }
                }
                v.push(&event_map[eid]);
                continue;
            }
            if let Some(eid_prev) = unconflicted.add_or_remove(t, s, eid) {
                let mut v = conflicted.get_mut_or_default(t, s);
                v.push(&event_map[eid]);
                v.push(&event_map[&eid_prev]);
            }
        }
    }

    let conflicted_senders: StateMap<HashSet<String>> = conflicted
        .iter()
        .map(|(key, event_ids)| {
            (key, event_ids.iter().map(|e| e.get_sender().into()).collect())
        })
        .collect();

    let state = resolve_conflicts_state(unconflicted, conflicted, event_map);

    let mut deltas = Vec::new();
    deltas.resize(state_sets.len(), StateMap::new());
    for (t, s) in conflicted_senders.keys() {
        if let Some(resolved_eid) = state.get(t, s) {
            for i in 0..state_sets.len() {
                if let Some(prev_eid) = state_sets[i].get(t, s) {
                    if prev_eid != resolved_eid {
                        deltas[i].insert(t, s, resolved_eid.clone());
                    }
                } else {
                    deltas[i].insert(t, s, resolved_eid.clone());
                }
            }
        }
    }

    let (delta_set, delta) = deltas
        .into_iter()
        .enumerate()
        .min_by_key(|(i, d)| (*i, d.len()))
        .unwrap_or_default();

    ResolvedState {
        state,
        conflicted_senders,
        delta_set,
        delta,
    }
}

// pub fn resolve_state_delta<E>(
//     state_sets: Vec<&StateMap<String>>,
//     event_map: &HashMap<String, E>,
//     prev_conflicted_senders: &StateMap<HashSet<String>>,
//     delta: &[(&str, &str)],
// ) -> ResolvedState
// where
//     E: EventV1,
// {
//     let to_recalculate =
//         auth::filter_changed_state(prev_conflicted_senders, delta);

//     let mut auth_types: HashSet<(String, String)> = to_recalculate
//         .iter()
//         .map(|(t, s)| ((*t).into(), (*s).into()))
//         .collect();

//     let mut pending: Vec<(String, String)> = to_recalculate
//         .iter()
//         .map(|(t, s)| ((*t).into(), (*s).into()))
//         .collect();
//     let mut handled = HashSet::new();
//     while let Some((t, s)) = pending.pop() {
//         if handled.contains(&(t.clone(), s.clone())) {
//             continue;
//         }
//         handled.insert((t.clone(), s.clone()));

//         for map in &state_sets {
//             if let Some(eid) = map.get(&t, &s) {
//                 let event = &event_map[eid];

//                 let a = auth::auth_types_for_event(event);
//                 for key in &a {
//                     if prev_conflicted_senders.get(&key.0, &key.1).is_some() {
//                         pending.push((key.0.clone(), key.1.clone()))
//                     }
//                 }

//                 auth_types.extend(a.into_iter());
//             }
//         }
//     }

//     let mut new_state_sets: Vec<StateMap<String>> = Vec::new();
//     new_state_sets.resize(state_sets.len(), StateMap::new());
//     for (t, s) in &auth_types {
//         for (i, set) in state_sets.iter().enumerate() {
//             if let Some(eid) = set.get(t, s) {
//                 new_state_sets[i].insert(t, s, eid.clone());
//             }
//         }
//     }

//     let mut r = resolve_state(new_state_sets.iter().collect(), event_map);

//     r.conflicted_senders
//         .extend(prev_conflicted_senders.iter().filter_map(
//             |((t, s), value)| {
//                 if !auth_types.contains(&(t.into(), s.into())) {
//                     Some(((t, s), value.clone()))
//                 } else {
//                     None
//                 }
//             },
//         ));

//     r
// }

fn resolve_conflicts_state<E>(
    unconflicted: StateMap<String>,
    conflicted: StateMap<SmallVec<[&E; 5]>>,
    event_map: &HashMap<String, E>,
) -> StateMap<String>
where
    E: EventV1,
{
    let mut auth_events_types = HashSet::new();
    for events in conflicted.values() {
        for event in events {
            auth_events_types
                .extend(auth::auth_types_for_event(*event).into_iter())
        }
    }

    let mut auth_events = StateMap::new();
    for (t, s) in auth_events_types {
        if let Some(evid) = unconflicted.get(&t, &s) {
            auth_events.insert(&t, &s, &event_map[evid as &str]);
        }
    }

    let mut resolved_state = unconflicted;

    if let Some(events) =
        conflicted.get_well_known(WellKnownEmptyKeys::PowerLevels)
    {
        let ev = resolve_auth_events(
            (WellKnownEmptyKeys::PowerLevels.as_str(), ""),
            events.to_vec(),
            &auth_events,
        );

        resolved_state.insert_well_known(
            WellKnownEmptyKeys::PowerLevels,
            ev.get_event_id().into(),
        );
        auth_events.insert_well_known(WellKnownEmptyKeys::PowerLevels, ev);
    }

    let join_auth_events = auth_events.clone();
    for (state_key, events) in conflicted.iter_join_rules() {
        let key = ("m.room.join_rules", state_key);
        let ev = resolve_auth_events(key, events.to_vec(), &join_auth_events);

        resolved_state.insert(key.0, key.1, ev.get_event_id().into());
        auth_events.insert(key.0, key.1, ev);
    }

    let member_auth_events = auth_events.clone();
    for (user, events) in conflicted.iter_members() {
        let key = ("m.room.member", user);
        let ev = resolve_auth_events(key, events.to_vec(), &member_auth_events);

        resolved_state.insert(key.0, key.1, ev.get_event_id().into());
        auth_events.insert(key.0, key.1, ev);
    }

    for (key, events) in conflicted.iter_non_members() {
        if !resolved_state.contains_key(key.0, key.1) {
            let ev = resolve_normal_events(events.to_vec(), &auth_events);

            resolved_state.insert(key.0, key.1, ev.get_event_id().into());
        }
    }

    resolved_state
}

fn resolve_auth_events<'a, E>(
    key: (&str, &str),
    mut events: Vec<&'a E>,
    auth_events: &StateMap<&'a E>,
) -> &'a E
where
    E: EventV1,
{
    order_events(&mut events);
    events.reverse();

    let mut new_auth_events = auth_events.clone();

    let key = (&key.0 as &str, &key.1 as &str);

    let mut prev_event = &events[0];
    for event in &events[1..] {
        new_auth_events.insert(key.0, key.1, prev_event);

        if auth::check(event, &new_auth_events).is_err() {
            return prev_event;
        }

        prev_event = event
    }

    return prev_event;
}

fn resolve_normal_events<'a, E>(
    mut events: Vec<&'a E>,
    auth_events: &StateMap<&'a E>,
) -> &'a E
where E: EventV1
{
    order_events(&mut events);

    for event in &events {
        if auth::check(event, &auth_events).is_ok() {
            return event;
        }
    }

    return events.last().unwrap();
}

fn order_events<E: EventV1>(events: &mut Vec<&E>) {
    events.sort_by_key(|e| {
        (-(e.get_depth()), Sha1::from(e.get_event_id()).hexdigest())
    })
}

#[test]
fn test_order_events() {
    use serde_json;

    let event1 = EventV1 {
        event_id: "@1:a".to_string(),
        depth: 1,

        etype: String::new(),
        state_key: None,
        prev_events: Vec::new(),
        room_id: String::new(),
        redacts: None,
        sender: String::new(),
        content: serde_json::Map::new(),
    };

    let event2 = EventV1 {
        event_id: "@2:a".to_string(),
        depth: 2,

        etype: String::new(),
        state_key: None,
        prev_events: Vec::new(),
        room_id: String::new(),
        redacts: None,
        sender: String::new(),
        content: serde_json::Map::new(),
    };

    let event3 = EventV1 {
        event_id: "@3:b".to_string(),
        depth: 2,

        etype: String::new(),
        state_key: None,
        prev_events: Vec::new(),
        room_id: String::new(),
        redacts: None,
        sender: String::new(),
        content: serde_json::Map::new(),
    };

    let mut vec = vec![&event1, &event2, &event3];

    order_events(&mut vec);

    assert_eq!(vec[0].event_id, event2.event_id);
    assert_eq!(vec[1].event_id, event3.event_id);
    assert_eq!(vec[2].event_id, event1.event_id);
}
