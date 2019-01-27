use crate::protocol::Event;

use std::fmt;

use failure::Error;


/// Check if the given event parses auth.
pub fn check<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: Event,
    S: Event + Clone + fmt::Debug,
{
    // TODO: Sig checks, can federate, size checks.

    let sender_domain = get_domain_from_id(event.get_sender())?;

    if event.get_type() == "m.room.create" {
        let room_domain = get_domain_from_id(event.get_room_id())?;
        ensure!(
            room_domain == sender_domain,
            "sender and room domains do not match"
        );
        return Ok(());
    }

    if !auth_events.contains_key("m.room.create", "") {
        bail!("No create event");
    }

    if event.get_type() == "m.room.aliases" {
        let state_key = if let Some(s) = event.get_state_key() {
            s
        } else {
            bail!("alias event must be state event");
        };

        ensure!(
            state_key == sender_domain,
            "alias state key and sender domain do not match"
        );
    }

    if event.get_type() == "m.room.member" {
        return check_membership(event, auth_events);
    }

    check_user_in_room(event, auth_events)?;

    if event.get_type() == "m.room.third_party_invite" {
        return check_third_party_invite(event, auth_events);
    }

    check_can_send_event(event, auth_events)?;

    if event.get_type() == "m.room.power_levels" {
        check_power_levels(event, auth_events)?;
    }

    if event.get_type() == "m.room.redaction" {
        check_redaction(event, auth_events)?;
    }

    Ok(())
}

fn check_third_party_invite<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: EventBase,
    S: EventBase + Clone + fmt::Debug,
{
    let user_level = get_user_power_level(event.get_sender(), auth_events);
    let invite_level = get_named_level("invite", auth_events).unwrap_or(0);

    if user_level < invite_level {
        bail!("user power level is less than invite level");
    } else {
        Ok(())
    }
}

fn check_membership<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: EventBase,
    S: EventBase + Clone + fmt::Debug,
{
    let membership = event.get_content()["membership"]
        .as_str()
        .ok_or_else(|| format_err!("missing membership key"))?;

    let state_key = if let Some(state_key) = event.get_state_key() {
        state_key
    } else {
        bail!("membership event must be state event");
    };

    if membership == "join" {
        if let Some(creation_event) = auth_events.get("m.room.create", "") {
            if Some(creation_event.borrow().get_event_id()) == event.get_single_prev_event_id() {
                let creator = creation_event
                    .borrow()
                    .get_content()
                    .get("creator")
                    .and_then(|v| v.as_str());
                if creator == Some(&state_key) {
                    return Ok(());
                }
            }
        }
    }

    // TODO: Can federate

    let (caller_in_room, caller_invited) =
        if let Some(ev) = auth_events.get("m.room.member", event.get_sender()) {
            let m = ev.borrow().get_content()["membership"]
                .as_str()
                .ok_or_else(|| format_err!("missing membership key"))?;
            (m == "join", m == "invite")
        } else {
            (false, false)
        };

    let (target_in_room, target_banned) =
        if let Some(ev) = auth_events.get("m.room.member", state_key) {
            let m = ev.borrow().get_content()["membership"]
                .as_str()
                .ok_or_else(|| format_err!("missing membership key"))?;
            (m == "join", m == "ban")
        } else {
            (false, false)
        };

    if membership == "invite" && event.get_content().contains_key("third_party_invite") {
        verify_third_party_invite(event, auth_events)?;

        if target_banned {
            bail!("target is banned");
        }
        return Ok(());
    }

    let join_rule = auth_events
        .get("m.room.join_rules", "")
        .and_then(|ev| ev.borrow().get_content().get("join_rule"))
        .and_then(Value::as_str)
        .unwrap_or("invite");

    let user_level = get_user_power_level(event.get_sender(), auth_events);
    let target_level = get_user_power_level(state_key, auth_events);

    let ban_level = get_named_level("ban", auth_events).unwrap_or(50);

    // TODO: third party invite

    if membership != "join" {
        if caller_invited && membership == "leave" && state_key == event.get_sender() {
            return Ok(());
        }

        if !caller_in_room {
            bail!("sender not in room");
        }
    }

    match membership {
        "invite" => {
            if target_banned {
                bail!("target is banned");
            }

            if target_in_room {
                bail!("target already in room");
            }

            if user_level < get_named_level("invite", auth_events).unwrap_or(0) {
                bail!("user power level is less than invite level");
            }
        }
        "join" => {
            if target_banned {
                bail!("user is banned");
            }
            if event.get_sender() != state_key {
                bail!("sender and state key do not match")
            }

            match join_rule {
                "public" => {}
                "invite" => {
                    if !caller_in_room && !caller_invited {
                        bail!("user not invited")
                    }
                }
                _ => bail!("unknown join rule"),
            }
        }
        "leave" => {
            if target_banned && user_level < ban_level {
                bail!("cannot unban user")
            }

            if state_key != event.get_sender() {
                let kick_level = get_named_level("kick", auth_events).unwrap_or(50);
                if user_level < kick_level || user_level <= target_level {
                    bail!("cannot kick user")
                }
            }
        }
        "ban" => {
            if user_level < ban_level || user_level <= target_level {
                bail!("cannot ban user")
            }
        }
        _ => bail!("unknown membership"),
    }

    Ok(())
}

fn check_user_in_room<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: Event,
    S: Event + Clone + fmt::Debug,
{
    let m = auth_events
        .get("m.room.member", event.get_sender())
        .and_then(|e| e.borrow().get_content().get("membership"))
        .and_then(Value::as_str);

    if m == Some("join") {
        Ok(())
    } else {
        bail!("user not in room");
    }
}

fn check_can_send_event<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: Event,
    S: Event + Clone + fmt::Debug,
{
    let send_level = get_send_level(
        event.get_type(),
        event.get_state_key().is_some(),
        auth_events,
    );
    let user_level = get_user_power_level(event.get_sender(), auth_events);

    if user_level < send_level {
        bail!("user doesn't have power to send event");
    }

    if let Some(state_key) = event.get_state_key() {
        if state_key.starts_with("@") && state_key != event.get_sender() {
            bail!("cannot have user state_key");
        }
    }

    Ok(())
}

fn check_power_levels<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: Event,
    S: Event + Clone + fmt::Debug,
{
    let current_power = if let Some(ev) = auth_events.get("m.room.power_levels", "") {
        ev
    } else {
        return Ok(());
    };

    let user_level = get_user_power_level(event.get_sender(), auth_events);

    let levels_to_check = vec![
        "users_default",
        "events_default",
        "state_default",
        "ban",
        "kick",
        "redact",
        "invite",
    ];

    for name in levels_to_check {
        let old_level = current_power
            .borrow()
            .get_content()
            .get(name)
            .and_then(as_int);
        let new_level = event.get_content().get(name).and_then(as_int);

        if old_level == new_level {
            continue;
        }

        if let Some(l) = old_level {
            if l > user_level {
                bail!("old level higher for {} greater than users", name);
            }
        }

        if let Some(l) = new_level {
            if l > user_level {
                bail!("new level higher for {} greater than users", name);
            }
        }
    }

    let old_users: HashMap<String, NumberLike> = current_power
        .borrow()
        .get_content()
        .get("users")
        .map(|v| {
            serde_json::from_value(v.clone()).map_err(|_| format_err!("invalid power level event"))
        }).map_or(Ok(None), |v| v.map(Some))?
        .unwrap_or_default();

    let new_users: HashMap<String, NumberLike> = event
        .get_content()
        .get("users")
        .map(|v| {
            serde_json::from_value(v.clone()).map_err(|_| format_err!("invalid power level event"))
        }).map_or(Ok(None), |v| v.map(Some))?
        .unwrap_or_default();

    let mut users_to_check = HashSet::new();
    users_to_check.extend(old_users.keys());
    users_to_check.extend(new_users.keys());

    for user in users_to_check {
        let old_level = old_users.get(user);
        let new_level = new_users.get(user);

        if old_level == new_level {
            continue;
        }

        if let Some(l) = old_level {
            if l.0 >= user_level {
                bail!("old level higher for {} greater than users", user);
            }
        }

        if let Some(l) = new_level {
            if l.0 > user_level {
                bail!("new level higher for {} greater than users", user);
            }
        }
    }

    let old_events: HashMap<String, NumberLike> = current_power
        .borrow()
        .get_content()
        .get("events")
        .map(|v| {
            serde_json::from_value(v.clone()).map_err(|_| format_err!("invalid power level event"))
        }).map_or(Ok(None), |v| v.map(Some))?
        .unwrap_or_default();

    let new_events: HashMap<String, NumberLike> = event
        .get_content()
        .get("events")
        .map(|v| {
            serde_json::from_value(v.clone()).map_err(|_| format_err!("invalid power level event"))
        }).map_or(Ok(None), |v| v.map(Some))?
        .unwrap_or_default();

    let mut events_to_check = HashSet::new();
    events_to_check.extend(old_events.keys());
    events_to_check.extend(new_events.keys());

    for etype in events_to_check {
        let old_level = old_events.get(etype);
        let new_level = new_events.get(etype);

        if old_level == new_level {
            continue;
        }

        if let Some(l) = old_level {
            if l.0 > user_level {
                bail!("new level higher for {} greater than users", etype);
            }
        }

        if let Some(l) = new_level {
            if l.0 > user_level {
                bail!("new level higher for {} greater than users", etype);
            }
        }
    }

    Ok(())
}

fn check_redaction<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: Event,
    S: Event + Clone + fmt::Debug,
{
    let user_level = get_user_power_level(event.get_sender(), auth_events);
    let redact_level = get_named_level("redact", auth_events).unwrap_or(50);

    if user_level >= redact_level {
        return Ok(());
    }

    if let Some(redacts) = event.get_redacts() {
        if get_domain_from_id(redacts)? == get_domain_from_id(event.get_sender())? {
            return Ok(());
        }
    }

    bail!("cannot redact");
}

fn verify_third_party_invite<E, S>(event: &E, auth_events: &StateMap<S>) -> Result<(), Error>
where
    E: Event,
    S: Event + Clone + fmt::Debug,
{
    let third_party = event
        .get_content()
        .get("third_party_invite")
        .ok_or_else(|| format_err!("not third party invite"))?;

    let signed_value = third_party
        .get("signed")
        .ok_or_else(|| format_err!("invalid third party invite"))?;

    let signed: ThridPartyInviteSigned = serde_json::from_value(signed_value.clone())
        .map_err(|_| format_err!("invalid third party invite"))?;

    let third_party_invite = auth_events
        .get("m.room.third_party_invite", &signed.token)
        .ok_or_else(|| format_err!("no third party invite event"))?;

    if third_party_invite.borrow().get_sender() != event.get_sender() {
        bail!("third party invite and event sender don't match");
    }

    if Some(&signed.mixd as &str) != event.get_state_key() {
        bail!("state_key and signed mxid do not match");
    }

    // TODO: Verify signature

    Ok(())
}

fn get_user_power_level<S: Event + Clone + fmt::Debug>(
    user: &str,
    auth_events: &StateMap<S>,
) -> i64 {
    if let Some(pev) = auth_events.get("m.room.power_levels", "") {
        let default = pev
            .borrow()
            .get_content()
            .get("users_default")
            .and_then(as_int)
            .unwrap_or(0);

        pev.borrow()
            .get_content()
            .get("users")
            .and_then(Value::as_object)
            .and_then(|u| u.get(user))
            .and_then(as_int)
            .unwrap_or(default)
    } else {
        auth_events
            .get("m.room.create", "")
            .and_then(|ev| ev.borrow().get_content().get("creator"))
            .and_then(Value::as_str)
            .map(|creator| if creator == user { 100 } else { 0 })
            .unwrap_or(0)
    }
}

fn get_named_level<S: Event + Clone + fmt::Debug>(
    name: &str,
    auth_events: &StateMap<S>,
) -> Option<i64> {
    auth_events
        .get("m.room.power_levels", "")
        .and_then(|ev| ev.borrow().get_content().get(name))
        .and_then(as_int)
}

fn get_send_level<S: Event + Clone + fmt::Debug>(
    etype: &str,
    is_state: bool,
    auth_events: &StateMap<S>,
) -> i64 {
    if let Some(pev) = auth_events.get("m.room.power_levels", "") {
        let default = if is_state {
            pev.borrow()
                .get_content()
                .get("state_default")
                .and_then(as_int)
                .unwrap_or(50)
        } else {
            pev.borrow()
                .get_content()
                .get("events_default")
                .and_then(as_int)
                .unwrap_or(0)
        };

        pev.borrow()
            .get_content()
            .get("events")
            .and_then(Value::as_object)
            .and_then(|u| u.get(etype))
            .and_then(as_int)
            .unwrap_or(default)
    } else {
        0
    }
}

fn as_int(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }

    if let Some(n) = value.as_f64() {
        // FIXME: Ermh?
        return Some(n as i64);
    }

    if let Some(s) = value.as_str() {
        return s.parse().ok();
    }

    None
}

pub fn auth_types_for_event(event: &Event) -> Vec<(String, String)> {
    if event.etype == "m.room.create" {
        return Vec::new();
    }

    let mut auth_types = vec![
        ("m.room.create".into(), "".into()),
        ("m.room.power_levels".into(), "".into()),
        ("m.room.member".into(), event.sender.clone()),
    ];

    if event.etype == "m.room.member" {
        let membership = event.content["membership"].as_str().unwrap_or_default(); // TODO: Is this ok?

        if membership == "join" || membership == "invite" {
            auth_types.push(("m.room.join_rules".into(), "".into()));
        }

        if let Some(ref state_key) = event.state_key {
            auth_types.push(("m.room.member".into(), state_key.clone()));
        }

        // TODO: Third party invite
    }

    auth_types
}
