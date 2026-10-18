//! Maps source identifiers to their source text.

use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

/// Stores owned source text indexed by unique identifiers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceMap {
    id_to_source: SourceStorage,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum SourceStorage {
    #[default]
    Empty,
    One(SourceId, String),
    Many(HashMap<SourceId, String>),
}

/// Identifies a source stored in a [`SourceMap`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(NonZeroU64);

impl SourceMap {
    /// Adds source text and returns its identifier.
    pub fn add_source(&mut self, source: impl Into<String>) -> SourceId {
        let id = SourceId::new_unique();
        let source = source.into();
        match &mut self.id_to_source {
            SourceStorage::Empty => self.id_to_source = SourceStorage::One(id, source),
            SourceStorage::One(source_id, previous_source) => {
                self.id_to_source = SourceStorage::Many(HashMap::from_iter([
                    (id, source),
                    (*source_id, std::mem::take(previous_source)),
                ]));
            }
            SourceStorage::Many(hash_map) => {
                hash_map.insert(id, source);
            }
        }
        id
    }

    /// Returns the source text for `id`, if present.
    pub fn get(&self, id: SourceId) -> Option<&str> {
        let source = match &self.id_to_source {
            SourceStorage::Empty => None,
            SourceStorage::One(source_id, source) => (*source_id == id).then_some(source),
            SourceStorage::Many(sources) => sources.get(&id),
        };
        if let Some(source) = source {
            return Some(source.as_ref());
        }
        None
    }
}

impl SourceId {
    /// Creates a new globally unique source identifier.
    pub fn new_unique() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        SourceId(NonZeroU64::new(id).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successive_ids_are_unique() {
        let first = SourceId::new_unique();
        let second = SourceId::new_unique();

        assert!(first != second);
    }

    #[test]
    fn many_new_ids_are_all_distinct() {
        let ids: Vec<SourceId> = (0..100).map(|_| SourceId::new_unique()).collect();

        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert!(ids[i] != ids[j], "duplicate ids at {i} and {j}");
            }
        }
    }

    #[test]
    fn empty_map_returns_none() {
        let map = SourceMap::default();

        assert_eq!(map.get(SourceId::new_unique()), None);
    }

    #[test]
    fn unknown_id_returns_none() {
        let mut map = SourceMap::default();
        map.add_source("hello");
        map.add_source(String::from("world"));

        assert_eq!(map.get(SourceId::new_unique()), None);
    }

    #[test]
    fn single_source_returns_inserted_content() {
        let mut map = SourceMap::default();
        let id = map.add_source("hello");

        assert_eq!(map.get(id), Some("hello"));
    }

    #[test]
    fn two_sources_both_return_correct_content() {
        let mut map = SourceMap::default();
        let first = map.add_source("first");
        let second = map.add_source(String::from("second"));

        assert_eq!(map.get(first), Some("first"));
        assert_eq!(map.get(second), Some("second"));
    }

    #[test]
    fn many_sources_all_return_correct_content() {
        let mut map = SourceMap::default();
        let mut expected = Vec::new();
        for i in 0..50 {
            let text = format!("source {i}");
            let id = map.add_source(text.clone());
            expected.push((id, text));
        }

        for (id, text) in &expected {
            assert_eq!(map.get(*id), Some(text.as_str()));
        }
    }

    #[test]
    fn empty_string_source_returns_some_empty() {
        let mut map = SourceMap::default();
        let id = map.add_source("");

        assert_eq!(map.get(id), Some(""));
    }
}
