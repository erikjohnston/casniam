#![allow(clippy::type_complexity)]

#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate failure;
#[cfg(test)]
#[macro_use]
extern crate serde_json;

#[track_caller]
macro_rules! expect_or_err {
    ($e:expr) => {
        if let Some(r) = $e {
            r
        } else {
            return Err(format_err!("'{}' was None", stringify!($e)).into());
        }
    };
}

pub mod json;
pub mod protocol;
pub mod state_map;
pub mod stores;

use std::borrow::Borrow;
use std::{fmt::Debug, iter::FromIterator};

use crate::protocol::{RoomState, StateMetadata};
use crate::state_map::StateMap;

#[derive(Debug, Clone)]
pub struct StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    map: StateMap<E>,
    metadata: StateMetadata,
}

impl<E> Default for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    fn default() -> Self {
        StateMapWithData {
            map: StateMap::new(),
            metadata: StateMetadata::default(),
        }
    }
}

impl<E> StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    fn new() -> Self {
        StateMapWithData::default()
    }

    fn insert(&mut self, t: &str, s: &str, value: E) {
        self.metadata.changed(t, s);
        self.map.insert(t, s, value);
    }
}

impl<E> RoomState<E> for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    fn new() -> Self {
        StateMapWithData::new()
    }

    fn add_event(&mut self, etype: String, state_key: String, event_id: E) {
        self.metadata.changed(&etype, &state_key);
        self.map.insert(&etype, &state_key, event_id);
    }

    fn remove(&mut self, etype: &str, state_key: &str) {
        self.metadata.changed(&etype, &state_key);
        self.map.remove(etype, state_key);
    }

    fn get(
        &self,
        event_type: impl Borrow<str>,
        state_key: impl Borrow<str>,
    ) -> Option<&E> {
        self.map.get(event_type.borrow(), state_key.borrow())
    }

    fn get_event_ids(
        &self,
        types: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<E> {
        types
            .into_iter()
            .filter_map(|(t, s)| self.map.get(&t, &s))
            .cloned()
            .collect()
    }

    fn keys(&self) -> Vec<(&str, &str)> {
        self.map.keys().collect()
    }

    fn values<'a>(&'a self) -> Box<dyn Iterator<Item = &'a E> + Send + 'a> {
        Box::new(self.map.values())
    }

    fn iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = ((&'a str, &'a str), &'a E)> + Send + 'a> {
        Box::new(self.map.iter())
    }

    fn metadata(&self) -> &StateMetadata {
        &self.metadata
    }

    fn mark_persisted(&mut self, sg: usize) {
        self.metadata.mark_persisted(sg);
    }
}

impl<E> FromIterator<((String, String), E)> for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    fn from_iter<T: IntoIterator<Item = ((String, String), E)>>(
        iter: T,
    ) -> StateMapWithData<E> {
        let map = iter.into_iter().collect();

        StateMapWithData {
            map,
            metadata: StateMetadata::default(),
        }
    }
}

impl<'a, E> FromIterator<((&'a str, &'a str), E)> for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    fn from_iter<T: IntoIterator<Item = ((&'a str, &'a str), E)>>(
        iter: T,
    ) -> StateMapWithData<E> {
        let map = iter.into_iter().collect();

        StateMapWithData {
            map,
            metadata: StateMetadata::default(),
        }
    }
}

impl<E> Extend<((String, String), E)> for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
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

impl<'a, E> Extend<((&'a str, &'a str), E)> for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
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

impl<E> IntoIterator for StateMapWithData<E>
where
    E: Clone + Debug + Send + Sync + 'static,
{
    type Item = ((String, String), E);
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + Send>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.into_iter()
    }
}

impl<E> PartialEq for StateMapWithData<E>
where
    E: PartialEq + Clone + Debug + Send + Sync + 'static,
{
    fn eq(&self, other: &Self) -> bool {
        self.map.eq(&other.map)
    }
}

impl<E> Eq for StateMapWithData<E> where
    E: Eq + PartialEq + Clone + Debug + Send + Sync + 'static
{
}
