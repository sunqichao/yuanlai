use std::collections::BTreeMap;

/// One entry change recorded by a [`Journal`]: undo restores `before`, redo restores `after`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change<K, V> {
    /// The key.
    pub key: K,
    /// Value before the block, `None` if absent.
    pub before: Option<V>,
    /// Value after the block, `None` if removed.
    pub after: Option<V>,
}

/// Copy-on-write view of a base map. Reads see pending writes; the base is never modified.
pub(crate) struct Journal<'a, K, V> {
    base: &'a BTreeMap<K, V>,
    changes: BTreeMap<K, (Option<V>, Option<V>)>,
}

impl<'a, K: Ord + Clone, V: Clone + PartialEq> Journal<'a, K, V> {
    pub(crate) const fn new(base: &'a BTreeMap<K, V>) -> Self {
        Self {
            base,
            changes: BTreeMap::new(),
        }
    }

    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        match self.changes.get(key) {
            Some((_, after)) => after.as_ref(),
            None => self.base.get(key),
        }
    }

    pub(crate) fn contains(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    pub(crate) fn set(&mut self, key: K, value: Option<V>) {
        let base = self.base;
        self.changes
            .entry(key)
            .or_insert_with_key(|k| (base.get(k).cloned(), None))
            .1 = value;
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        self.set(key, Some(value));
    }

    pub(crate) fn remove(&mut self, key: K) {
        self.set(key, None);
    }

    /// Entries with keys `≥ start` for which `within` holds, in key order. `within` must be
    /// monotone: once it fails for a key it fails for all larger keys.
    pub(crate) fn scan(&self, start: &K, within: impl Fn(&K) -> bool) -> Vec<(K, V)> {
        let mut merged: BTreeMap<K, V> = self
            .base
            .range(start..)
            .take_while(|(k, _)| within(k))
            .filter(|(k, _)| !self.changes.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (k, (_, after)) in self.changes.range(start..).take_while(|(k, _)| within(k)) {
            if let Some(v) = after {
                merged.insert(k.clone(), v.clone());
            }
        }
        merged.into_iter().collect()
    }

    pub(crate) fn finish(self) -> Vec<Change<K, V>> {
        self.changes
            .into_iter()
            .filter(|(_, (before, after))| before != after)
            .map(|(key, (before, after))| Change { key, before, after })
            .collect()
    }
}

/// Applies (`forward`) or reverts recorded changes to `map`.
pub(crate) fn replay<K: Ord + Clone, V: Clone>(
    map: &mut BTreeMap<K, V>,
    changes: &[Change<K, V>],
    forward: bool,
) {
    for change in changes {
        let value = if forward {
            &change.after
        } else {
            &change.before
        };
        match value {
            Some(v) => {
                map.insert(change.key.clone(), v.clone());
            }
            None => {
                map.remove(&change.key);
            }
        }
    }
}
