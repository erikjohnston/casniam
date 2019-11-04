use std::borrow::Borrow;
use std::collections::{hash_map, HashMap};
use std::fmt::Debug;
use std::iter::FromIterator;
use std::str::FromStr;

const TYPE_CREATE: &str = "m.room.create";
const TYPE_POWER_LEVELS: &str = "m.room.power_levels";
const TYPE_JOIN_RULES: &str = "m.room.join_rules";
const TYPE_HISTORY_VISIBILITY: &str = "m.room.history_visibility";
const TYPE_NAME: &str = "m.room.name";
const TYPE_TOPIC: &str = "m.room.topic";
const TYPE_AVATAR: &str = "m.room.avatar";
const TYPE_GUEST_ACCESS: &str = "m.room.guest_access";
const TYPE_CANONICAL_ALIASES: &str = "m.room.canonical_alias";
const TYPE_RELATED_GROUPS: &str = "m.room.related_groups";
const TYPE_ENCRYPTION: &str = "m.room.encryption";

const TYPE_MEMBERSHIP: &str = "m.room.member";
const TYPE_ALIASES: &str = "m.room.aliases";
const TYPE_THIRD_PARTY_INVITE: &str = "m.room.third_party_invite";

/// List of event types that are commonly used for state with empty state
/// keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WellKnownEmptyKeys {
    Create,
    PowerLevels,
    JoinRules,
    HistoryVisibility,
    Name,
    Topic,
    Avatar,
    GuestAccess,
    CanonicalAliases,
    RelatedGroups,
    Encryption,
}

impl WellKnownEmptyKeys {
    pub fn as_str(self) -> &'static str {
        match self {
            WellKnownEmptyKeys::Create => TYPE_CREATE,
            WellKnownEmptyKeys::PowerLevels => TYPE_POWER_LEVELS,
            WellKnownEmptyKeys::JoinRules => TYPE_JOIN_RULES,
            WellKnownEmptyKeys::HistoryVisibility => TYPE_HISTORY_VISIBILITY,
            WellKnownEmptyKeys::Name => TYPE_NAME,
            WellKnownEmptyKeys::Topic => TYPE_TOPIC,
            WellKnownEmptyKeys::Avatar => TYPE_AVATAR,
            WellKnownEmptyKeys::GuestAccess => TYPE_GUEST_ACCESS,
            WellKnownEmptyKeys::CanonicalAliases => TYPE_CANONICAL_ALIASES,
            WellKnownEmptyKeys::RelatedGroups => TYPE_RELATED_GROUPS,
            WellKnownEmptyKeys::Encryption => TYPE_ENCRYPTION,
        }
    }
}

impl FromStr for WellKnownEmptyKeys {
    type Err = failure::Error;

    fn from_str(t: &str) -> Result<Self, failure::Error> {
        match t {
            TYPE_CREATE => Ok(WellKnownEmptyKeys::Create),
            TYPE_POWER_LEVELS => Ok(WellKnownEmptyKeys::PowerLevels),
            TYPE_JOIN_RULES => Ok(WellKnownEmptyKeys::JoinRules),
            TYPE_HISTORY_VISIBILITY => {
                Ok(WellKnownEmptyKeys::HistoryVisibility)
            }
            TYPE_NAME => Ok(WellKnownEmptyKeys::Name),
            TYPE_TOPIC => Ok(WellKnownEmptyKeys::Topic),
            TYPE_AVATAR => Ok(WellKnownEmptyKeys::Avatar),
            TYPE_GUEST_ACCESS => Ok(WellKnownEmptyKeys::GuestAccess),
            TYPE_CANONICAL_ALIASES => Ok(WellKnownEmptyKeys::CanonicalAliases),
            TYPE_RELATED_GROUPS => Ok(WellKnownEmptyKeys::RelatedGroups),
            TYPE_ENCRYPTION => Ok(WellKnownEmptyKeys::Encryption),
            _ => Err(format_err!("Not a valid well_known")),
        }
    }
}

/// A specialised container for storing state mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateMap<E: Debug + Clone> {
    well_known: HashMap<WellKnownEmptyKeys, E>,
    membership: HashMap<String, E>,
    aliases: HashMap<String, E>,
    invites: HashMap<String, E>,
    others: HashMap<String, HashMap<String, E>>,
}

impl<E> StateMap<E>
where
    E: Debug + Clone,
{
    pub fn new() -> StateMap<E> {
        StateMap {
            well_known: HashMap::new(),
            membership: HashMap::new(),
            aliases: HashMap::new(),
            invites: HashMap::new(),
            others: HashMap::new(),
        }
    }

    pub fn get_well_known(&self, key: WellKnownEmptyKeys) -> Option<&E> {
        self.well_known.get(&key)
    }

    pub fn get_aliases(&self, server: &str) -> Option<&E> {
        self.aliases.get(server)
    }

    pub fn get_membership(&self, server: &str) -> Option<&E> {
        self.membership.get(server)
    }

    pub fn get_third_party_invites(&self, token: &str) -> Option<&E> {
        self.invites.get(token)
    }

    pub fn get(&self, t: &str, s: &str) -> Option<&E> {
        if s == "" {
            if let Ok(key) = WellKnownEmptyKeys::from_str(t) {
                return self.get_well_known(key);
            }
        }

        match (t.borrow(), s.borrow()) {
            (TYPE_MEMBERSHIP, user) => self.get_membership(user),
            (TYPE_ALIASES, server) => self.get_aliases(server),
            (TYPE_THIRD_PARTY_INVITE, token) => {
                self.get_third_party_invites(token)
            }

            (t, s) => self.others.get(t).and_then(|m| m.get(s)),
        }
    }

    pub fn get_mut(&mut self, t: &str, s: &str) -> Option<&mut E> {
        if s == "" {
            if let Ok(key) = WellKnownEmptyKeys::from_str(t) {
                return self.well_known.get_mut(&key);
            }
        }

        match (t.borrow(), s.borrow()) {
            (TYPE_MEMBERSHIP, user) => self.membership.get_mut(user),
            (TYPE_ALIASES, server) => self.aliases.get_mut(server),
            (TYPE_THIRD_PARTY_INVITE, token) => self.invites.get_mut(token),

            (t, s) => self.others.get_mut(t).and_then(|m| m.get_mut(s)),
        }
    }

    pub fn insert_well_known(&mut self, k: WellKnownEmptyKeys, value: E) {
        self.well_known.insert(k, value);
    }

    pub fn insert(&mut self, t: &str, s: &str, value: E) {
        if s == "" {
            if let Ok(key) = WellKnownEmptyKeys::from_str(t) {
                self.well_known.insert(key, value);
                return;
            }
        }

        match (t, s) {
            (TYPE_MEMBERSHIP, user) => {
                self.membership.insert(user.into(), value)
            }
            (TYPE_ALIASES, server) => self.aliases.insert(server.into(), value),
            (TYPE_THIRD_PARTY_INVITE, token) => {
                self.invites.insert(token.into(), value)
            }

            (t, s) => self
                .others
                .entry(t.into())
                .or_insert_with(HashMap::new)
                .insert(s.into(), value),
        };
    }

    pub fn remove(&mut self, t: &str, s: &str) {
        if s == "" {
            if let Ok(key) = WellKnownEmptyKeys::from_str(t) {
                self.well_known.remove(&key);
                return;
            }
        }

        match (t, s) {
            (TYPE_MEMBERSHIP, user) => self.membership.remove(user),
            (TYPE_ALIASES, server) => self.aliases.remove(server),
            (TYPE_THIRD_PARTY_INVITE, token) => self.invites.remove(token),

            (t, s) => self.others.get_mut(t).and_then(|m| m.remove(s)),
        };
    }

    pub fn contains_key(&self, t: &str, s: &str) -> bool {
        self.get(t, s).is_some()
    }

    pub fn keys(&self) -> impl Iterator<Item = (&str, &str)> {
        let well_known = self.well_known.keys().map(|k| (k.as_str(), ""));

        let members =
            self.membership.keys().map(|u| (TYPE_MEMBERSHIP, u as &str));

        let aliases = self.aliases.keys().map(|s| (TYPE_ALIASES, s as &str));

        let invites = self
            .invites
            .keys()
            .map(|t| (TYPE_THIRD_PARTY_INVITE, t as &str));

        let others = self
            .others
            .iter()
            .flat_map(|(t, h)| h.keys().map(move |s| (t as &str, s as &str)));

        well_known
            .chain(members)
            .chain(aliases)
            .chain(invites)
            .chain(others)
    }

    pub fn iter(&self) -> impl Iterator<Item = ((&str, &str), &E)> {
        let well_known =
            self.well_known.iter().map(|(k, e)| ((k.as_str(), ""), e));

        let members = self
            .membership
            .iter()
            .map(|(u, e)| ((TYPE_MEMBERSHIP, u as &str), e));

        let aliases = self
            .aliases
            .iter()
            .map(|(s, e)| ((TYPE_ALIASES, s as &str), e));

        let invites = self
            .invites
            .iter()
            .map(|(t, e)| ((TYPE_THIRD_PARTY_INVITE, t as &str), e));

        let others = self.others.iter().flat_map(|(t, h)| {
            h.iter().map(move |(s, e)| ((t as &str, s as &str), e))
        });

        well_known
            .chain(members)
            .chain(aliases)
            .chain(invites)
            .chain(others)
    }

    pub fn values(&self) -> impl Iterator<Item = &E> {
        let well_known = self.well_known.values();

        let members = self.membership.values();

        let aliases = self.aliases.values();

        let invites = self.invites.values();

        let others = self.others.values().flat_map(HashMap::values);

        well_known
            .chain(members)
            .chain(aliases)
            .chain(invites)
            .chain(others)
    }

    pub fn iter_members(&self) -> impl Iterator<Item = (&str, &E)> {
        self.membership.iter().map(|(u, e)| (u as &str, e))
    }

    pub fn iter_join_rules(&self) -> impl Iterator<Item = (&str, &E)> {
        let i = self
            .well_known
            .get(&WellKnownEmptyKeys::JoinRules)
            .into_iter()
            .map(|e| ("", e));

        let o = self
            .others
            .get(TYPE_JOIN_RULES)
            .into_iter()
            .flat_map(|h| h.iter().map(move |(s, e)| (s as &str, e)));

        i.chain(o)
    }

    pub fn iter_non_members(&self) -> impl Iterator<Item = ((&str, &str), &E)> {
        let w = self.well_known.iter().map(|(k, e)| ((k.as_str(), ""), e));

        let a = self
            .aliases
            .iter()
            .map(|(s, e)| ((TYPE_ALIASES, s as &str), e));

        let i = self
            .invites
            .iter()
            .map(|(t, e)| ((TYPE_THIRD_PARTY_INVITE, t as &str), e));

        let o = self.others.iter().flat_map(|(t, h)| {
            h.iter().map(move |(s, e)| ((t as &str, s as &str), e))
        });

        w.chain(a).chain(i).chain(o)
    }

    pub fn len(&self) -> usize {
        let others: usize = self.others.values().map(HashMap::len).sum();
        self.well_known.len()
            + self.membership.len()
            + self.aliases.len()
            + self.invites.len()
            + others
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<E> StateMap<E>
where
    E: Debug + Clone + Default,
{
    pub fn get_mut_or_default(&mut self, t: &str, s: &str) -> &mut E {
        if s == "" {
            if let Ok(key) = WellKnownEmptyKeys::from_str(t) {
                return self.well_known.entry(key).or_insert_with(E::default);
            }
        }

        if let Some(entry) = match (t, s) {
            (TYPE_MEMBERSHIP, user) => Some(self.membership.entry(user.into())),
            (TYPE_ALIASES, server) => Some(self.aliases.entry(server.into())),
            (TYPE_THIRD_PARTY_INVITE, token) => {
                Some(self.invites.entry(token.into()))
            }

            _ => None,
        } {
            entry.or_insert_with(E::default)
        } else {
            self.others
                .entry(t.into())
                .or_insert_with(HashMap::new)
                .entry(s.into())
                .or_insert_with(E::default)
        }
    }
}

impl<E> StateMap<E>
where
    E: Debug + Clone + PartialEq,
{
    pub fn add_or_remove<F>(&mut self, t: &str, s: &str, v: F) -> Option<E>
    where
        F: Borrow<E>,
    {
        let value = v.borrow();
        if s == "" {
            if let Ok(key) = WellKnownEmptyKeys::from_str(t) {
                match self.well_known.entry(key) {
                    hash_map::Entry::Occupied(o) => {
                        if o.get() != value {
                            return Some(o.remove());
                        } else {
                            return None;
                        }
                    }
                    hash_map::Entry::Vacant(v) => {
                        v.insert(value.clone());
                        return None;
                    }
                }
            }
        }

        if let Some(entry) = match (t, s) {
            (TYPE_MEMBERSHIP, user) => Some(self.membership.entry(user.into())),
            (TYPE_ALIASES, server) => Some(self.aliases.entry(server.into())),
            (TYPE_THIRD_PARTY_INVITE, token) => {
                Some(self.invites.entry(token.into()))
            }

            _ => None,
        } {
            match entry {
                hash_map::Entry::Occupied(o) => {
                    if o.get() != value {
                        Some(o.remove())
                    } else {
                        None
                    }
                }
                hash_map::Entry::Vacant(v) => {
                    v.insert(value.clone());
                    None
                }
            }
        } else {
            match self.others.entry(t.into()) {
                hash_map::Entry::Occupied(mut o) => {
                    match o.get_mut().entry(s.into()) {
                        hash_map::Entry::Occupied(o) => {
                            if o.get() != value {
                                Some(o.remove())
                            } else {
                                None
                            }
                        }
                        hash_map::Entry::Vacant(v) => {
                            v.insert(value.clone());
                            None
                        }
                    }
                }
                hash_map::Entry::Vacant(v) => {
                    v.insert(HashMap::new()).insert(s.into(), value.clone());
                    None
                }
            }
        }
    }
}

impl<E> FromIterator<((String, String), E)> for StateMap<E>
where
    E: Debug + Clone,
{
    fn from_iter<T: IntoIterator<Item = ((String, String), E)>>(
        iter: T,
    ) -> StateMap<E> {
        let mut state_map = StateMap::new();

        for ((t, s), e) in iter {
            state_map.insert(&t, &s, e);
        }

        state_map
    }
}

impl<'a, E> FromIterator<((&'a str, &'a str), E)> for StateMap<E>
where
    E: Debug + Clone,
{
    fn from_iter<T: IntoIterator<Item = ((&'a str, &'a str), E)>>(
        iter: T,
    ) -> StateMap<E> {
        let mut state_map = StateMap::new();

        for ((t, s), e) in iter {
            state_map.insert(&t, &s, e);
        }

        state_map
    }
}

impl<E> Extend<((String, String), E)> for StateMap<E>
where
    E: Debug + Clone,
{
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = ((String, String), E)>,
    {
        for ((t, s), e) in iter {
            self.insert(&t, &s, e);
        }
    }
}

impl<'a, E> Extend<((&'a str, &'a str), E)> for StateMap<E>
where
    E: Debug + Clone,
{
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = ((&'a str, &'a str), E)>,
    {
        for ((t, s), e) in iter {
            self.insert(t, s, e);
        }
    }
}

impl<E> IntoIterator for StateMap<E>
where
    E: Debug + Clone + 'static,
{
    type Item = ((String, String), E);
    type IntoIter = Box<dyn Iterator<Item = Self::Item>>;

    fn into_iter(self) -> Self::IntoIter {
        let StateMap {
            well_known,
            membership,
            aliases,
            invites,
            others,
        } = self;

        let well_known = well_known
            .into_iter()
            .map(|(k, e)| ((k.as_str().to_string(), "".to_string()), e));

        let members = membership
            .into_iter()
            .map(|(u, e)| ((TYPE_MEMBERSHIP.to_string(), u), e));

        let aliases = aliases
            .into_iter()
            .map(|(s, e)| ((TYPE_ALIASES.to_string(), s), e));

        let invites = invites
            .into_iter()
            .map(|(t, e)| ((TYPE_THIRD_PARTY_INVITE.to_string(), t), e));

        let others = others.into_iter().flat_map(|(t, h)| {
            h.into_iter().map(move |(s, e)| ((t.clone(), s), e))
        });

        Box::new(
            well_known
                .chain(members)
                .chain(aliases)
                .chain(invites)
                .chain(others),
        )
    }
}

#[test]
fn add_or_remove_test() {
    let mut state_map = StateMap::new();

    for &(t, s) in &[
        ("test", "test2"),
        (TYPE_POWER_LEVELS, ""),
        (TYPE_POWER_LEVELS, "foo"),
        (TYPE_MEMBERSHIP, "foo"),
    ] {
        state_map.insert(t, s, 1);

        let res = state_map.add_or_remove(t, s, 2);
        assert_eq!(res, Some(1));

        assert_eq!(state_map.get(t, s), None);

        let res = state_map.add_or_remove(t, s, 1);
        assert_eq!(res, None);

        let res = state_map.add_or_remove(t, s, 1);
        assert_eq!(res, None);
    }
}
