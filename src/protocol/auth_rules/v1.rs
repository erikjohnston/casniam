use std::collections::{HashMap, HashSet};
use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::str::FromStr;

use failure::Error;

use futures::Future;
use serde_json::{self, Value};

use crate::protocol::AuthRules;
use crate::protocol::{Event, RoomState, RoomVersion};
use crate::state_map::StateMap;
use crate::stores::EventStore;

pub fn get_domain_from_id(string: &str) -> Result<&str, Error> {
    string
        .splitn(2, ':')
        .nth(1)
        .ok_or_else(|| format_err!("invalid ID: {}", string))
}

#[derive(Default)]
pub struct AuthV1<R> {
    e: PhantomData<R>,
}

impl<R> AuthRules for AuthV1<R>
where
    R: RoomVersion,
{
    type RoomVersion = R;

    fn check<'a, S: RoomState>(
        e: &'a R::Event,
        s: &'a S,
        store: &'a (impl EventStore<Self::RoomVersion, S> + Clone),
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> {
        Pin::from(Box::new(check(e.clone(), s.clone(), store.clone())))
    }

    fn auth_types_for_event(
        event_type: &str,
        state_key: Option<&str>,
        sender: &str,
        content: &serde_json::Map<String, Value>,
    ) -> Vec<(String, String)> {
        auth_types_for_event(event_type, state_key, sender, content)
    }
}

/// Check if the given event parses auth.
pub async fn check<R, S>(
    event: R::Event,
    state: S,
    store: impl EventStore<R, S>,
) -> Result<(), Error>
where
    R: RoomVersion,
    S: RoomState + Clone,
{
    let types = auth_types_for_event(
        event.event_type(),
        event.state_key(),
        event.sender(),
        event.content(),
    );
    let auth_event_ids = state.get_event_ids(types);

    let auth_events_vec = store
        .get_events(
            &auth_event_ids.iter().map(|e| e as &str).collect::<Vec<_>>(),
        )
        .await?;

    let auth_events = auth_events_vec
        .into_iter()
        .filter_map(|ev| {
            if let Some(state_key) = ev.state_key() {
                Some(((ev.event_type().to_string(), state_key.to_string()), ev))
            } else {
                None
            }
        })
        .collect();

    Checker {
        event: &event,
        auth_events,
    }
    .check()
}

struct Checker<'a, E: Clone + fmt::Debug> {
    event: &'a E,
    auth_events: StateMap<E>,
}

impl<'a, E> Checker<'a, E>
where
    E: Event,
{
    pub fn check(&'a self) -> Result<(), Error> {
        // TODO: Sig checks, can federate, size checks.

        let sender_domain = get_domain_from_id(self.event.sender())?;

        if self.event.event_type() == "m.room.create" {
            let room_domain = get_domain_from_id(self.event.room_id())?;
            ensure!(
                room_domain == sender_domain,
                "sender and room domains do not match"
            );
            return Ok(());
        }

        if self.auth_events.get("m.room.create", "").is_none() {
            bail!("No create event");
        }

        if self.event.event_type() == "m.room.aliases" {
            let state_key = if let Some(s) = self.event.state_key() {
                s
            } else {
                bail!("alias event must be state event");
            };

            ensure!(
                state_key == sender_domain,
                "alias state key and sender domain do not match"
            );
        }

        if self.event.event_type() == "m.room.member" {
            return self.check_membership();
        }

        self.check_user_in_room()?;

        if self.event.event_type() == "m.room.third_party_invite" {
            return self.check_third_party_invite();
        }

        self.check_can_send_event()?;

        if self.event.event_type() == "m.room.power_levels" {
            self.check_power_levels()?;
        }

        if self.event.event_type() == "m.room.redaction" {
            self.check_redaction()?;
        }

        Ok(())
    }

    fn check_third_party_invite(&self) -> Result<(), Error> {
        let user_level = self.get_user_power_level(self.event.sender());
        let invite_level = self.get_named_level("invite").unwrap_or(0);

        if user_level < invite_level {
            bail!("user power level is less than invite level");
        } else {
            Ok(())
        }
    }

    fn check_membership(&self) -> Result<(), Error> {
        let membership = self.event.content()["membership"]
            .as_str()
            .ok_or_else(|| format_err!("missing membership key"))?;

        let state_key = if let Some(state_key) = self.event.state_key() {
            state_key
        } else {
            bail!("membership event must be state event");
        };

        if membership == "join" {
            if let Some(creation_event) =
                self.auth_events.get("m.room.create", "")
            {
                let prev_events = self.event.prev_event_ids();
                let single_prev_event_id = if prev_events.len() == 1 {
                    Some(prev_events[0])
                } else {
                    None
                };

                if Some(creation_event.event_id()) == single_prev_event_id {
                    let creator = creation_event
                        .content()
                        .get("creator")
                        .and_then(Value::as_str);
                    if creator == Some(&state_key) {
                        return Ok(());
                    }
                }
            }
        }

        // TODO: Can federate

        let (caller_in_room, caller_invited) = if let Some(ev) =
            self.auth_events.get("m.room.member", self.event.sender())
        {
            let m = ev.content()["membership"]
                .as_str()
                .ok_or_else(|| format_err!("missing membership key"))?;
            (m == "join", m == "invite")
        } else {
            (false, false)
        };

        let (target_in_room, target_banned) = if let Some(ev) =
            self.auth_events.get("m.room.member", state_key)
        {
            let m = ev.content()["membership"]
                .as_str()
                .ok_or_else(|| format_err!("missing membership key"))?;
            (m == "join", m == "ban")
        } else {
            (false, false)
        };

        if membership == "invite"
            && self.event.content().contains_key("third_party_invite")
        {
            self.verify_third_party_invite()?;

            if target_banned {
                bail!("target is banned");
            }
            return Ok(());
        }

        let join_rule = self
            .auth_events
            .get("m.room.join_rules", "")
            .and_then(|ev| ev.content().get("join_rule"))
            .and_then(Value::as_str)
            .unwrap_or("invite");

        let user_level = self.get_user_power_level(self.event.sender());
        let target_level = self.get_user_power_level(state_key);

        let ban_level = self.get_named_level("ban").unwrap_or(50);

        // TODO: third party invite

        if membership != "join" {
            if caller_invited
                && membership == "leave"
                && state_key == self.event.sender()
            {
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

                if user_level < self.get_named_level("invite").unwrap_or(0) {
                    bail!("user power level is less than invite level");
                }
            }
            "join" => {
                if target_banned {
                    bail!("user is banned");
                }
                if self.event.sender() != state_key {
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

                if state_key != self.event.sender() {
                    let kick_level = self.get_named_level("kick").unwrap_or(50);
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

    fn check_user_in_room(&self) -> Result<(), Error> {
        let m = self
            .auth_events
            .get("m.room.member", self.event.sender())
            .and_then(|e| e.content().get("membership"))
            .and_then(Value::as_str);

        if m == Some("join") {
            Ok(())
        } else {
            bail!("user not in room");
        }
    }

    fn check_can_send_event(&self) -> Result<(), Error> {
        let send_level = self.get_send_level(
            self.event.event_type(),
            self.event.state_key().is_some(),
        );
        let user_level = self.get_user_power_level(self.event.sender());

        if user_level < send_level {
            bail!("user doesn't have power to send event");
        }

        if let Some(state_key) = self.event.state_key() {
            if state_key.starts_with('@') && state_key != self.event.sender() {
                bail!("cannot have user state_key");
            }
        }

        Ok(())
    }

    fn check_power_levels(&self) -> Result<(), Error> {
        let current_power =
            if let Some(ev) = self.auth_events.get("m.room.power_levels", "") {
                ev
            } else {
                return Ok(());
            };

        let user_level = self.get_user_power_level(self.event.sender());

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
            let old_level = current_power.content().get(name).and_then(as_int);
            let new_level = self.event.content().get(name).and_then(as_int);

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
            .content()
            .get("users")
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|_| format_err!("invalid power level event"))
            })
            .map_or(Ok(None), |v| v.map(Some))?
            .unwrap_or_default();

        let new_users: HashMap<String, NumberLike> = self
            .event
            .content()
            .get("users")
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|_| format_err!("invalid power level event"))
            })
            .map_or(Ok(None), |v| v.map(Some))?
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
                if l.0 > user_level {
                    bail!(
                        "old level higher for {} greater than users: {} > {}",
                        user,
                        l.0,
                        user_level
                    );
                }
            }

            if let Some(l) = new_level {
                if l.0 > user_level {
                    bail!("new level higher for {} greater than users", user);
                }
            }
        }

        let old_events: HashMap<String, NumberLike> = current_power
            .content()
            .get("events")
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|_| format_err!("invalid power level event"))
            })
            .map_or(Ok(None), |v| v.map(Some))?
            .unwrap_or_default();

        let new_events: HashMap<String, NumberLike> = self
            .event
            .content()
            .get("events")
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|_| format_err!("invalid power level event"))
            })
            .map_or(Ok(None), |v| v.map(Some))?
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

    fn check_redaction(&self) -> Result<(), Error> {
        let user_level = self.get_user_power_level(self.event.sender());
        let redact_level = self.get_named_level("redact").unwrap_or(50);

        if user_level >= redact_level {
            return Ok(());
        }

        if let Some(redacts) = self.event.redacts() {
            if get_domain_from_id(redacts)?
                == get_domain_from_id(self.event.sender())?
            {
                return Ok(());
            }
        }

        bail!("cannot redact");
    }

    fn verify_third_party_invite(&self) -> Result<(), Error> {
        let third_party = self
            .event
            .content()
            .get("third_party_invite")
            .ok_or_else(|| format_err!("not third party invite"))?;

        let signed_value = third_party
            .get("signed")
            .ok_or_else(|| format_err!("invalid third party invite"))?;

        let signed: ThridPartyInviteSigned =
            serde_json::from_value(signed_value.clone())
                .map_err(|_| format_err!("invalid third party invite"))?;

        let third_party_invite = self
            .auth_events
            .get("m.room.third_party_invite", &signed.token)
            .ok_or_else(|| format_err!("no third party invite event"))?;

        if third_party_invite.sender() != self.event.sender() {
            bail!("third party invite and event sender don't match");
        }

        if Some(&signed.mixd as &str) != self.event.state_key() {
            bail!("state_key and signed mxid do not match");
        }

        // TODO: Verify signature

        Ok(())
    }

    fn get_user_power_level(&self, user: &str) -> i64 {
        if let Some(pev) = self.auth_events.get("m.room.power_levels", "") {
            let default = pev
                .content()
                .get("users_default")
                .and_then(as_int)
                .unwrap_or(0);

            pev.content()
                .get("users")
                .and_then(Value::as_object)
                .and_then(|u| u.get(user))
                .and_then(as_int)
                .unwrap_or(default)
        } else {
            self.auth_events
                .get("m.room.create", "")
                .and_then(|ev| ev.content().get("creator"))
                .and_then(Value::as_str)
                .map(|creator| if creator == user { 100 } else { 0 })
                .unwrap_or(0)
        }
    }

    fn get_named_level(&self, name: &str) -> Option<i64> {
        self.auth_events
            .get("m.room.power_levels", "")
            .and_then(|ev| ev.content().get(name))
            .and_then(as_int)
    }

    fn get_send_level(&self, etype: &str, is_state: bool) -> i64 {
        if let Some(pev) = self.auth_events.get("m.room.power_levels", "") {
            let default = if is_state {
                pev.content()
                    .get("state_default")
                    .and_then(as_int)
                    .unwrap_or(50)
            } else {
                pev.content()
                    .get("events_default")
                    .and_then(as_int)
                    .unwrap_or(0)
            };

            pev.content()
                .get("events")
                .and_then(Value::as_object)
                .and_then(|u| u.get(etype))
                .and_then(as_int)
                .unwrap_or(default)
        } else {
            0
        }
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

pub fn auth_types_for_event(
    event_type: &str,
    state_key: Option<&str>,
    sender: &str,
    content: &serde_json::Map<String, Value>,
) -> Vec<(String, String)> {
    if event_type == "m.room.create" {
        return Vec::new();
    }

    let mut auth_types: Vec<(String, String)> = vec![
        ("m.room.create".into(), "".into()),
        ("m.room.power_levels".into(), "".into()),
        ("m.room.member".into(), sender.into()),
    ];

    if event_type == "m.room.member" {
        let membership = content["membership"].as_str().unwrap_or_default(); // TODO: Is this ok?

        if membership == "join" || membership == "invite" {
            auth_types.push(("m.room.join_rules".into(), "".into()));
        }

        if let Some(state_key) = state_key {
            auth_types.push(("m.room.member".into(), state_key.into()));
        }

        // TODO: Third party invite
    }

    auth_types
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NumberLike(#[serde(deserialize_with = "from_str")] i64);

fn from_str<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserializer.deserialize_i64(DeserializeU64OrStringVisitor)
}

struct DeserializeU64OrStringVisitor;

impl<'de> serde::de::Visitor<'de> for DeserializeU64OrStringVisitor {
    type Value = i64;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an integer or a string")
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v as i64)
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        i64::from_str(&v).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ThridPartyInviteSigned {
    mixd: String,
    sender: String,
    token: String,
}
