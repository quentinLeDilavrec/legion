use std::{
    borrow::Borrow,
    collections::{hash_map::Entry, HashMap},
    hash::Hash,
};

use crate::id::AppendId;

/// The outcome of an insertion that may be refused because the key already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    /// A new entity was created.
    New(AppendId),
    /// An entity with the same key already existed; nothing was inserted.
    Existing(AppendId),
}

impl Inserted {
    /// The new entity, or the one that already had the key.
    pub fn id(self) -> AppendId {
        match self {
            Self::New(id) | Self::Existing(id) => id,
        }
    }

    pub fn is_new(self) -> bool {
        matches!(self, Self::New(_))
    }
}

/// A secondary index from a key to the one entity that has it.
///
/// Since entity IDs never change in an append-only world, the index never goes stale because of
/// entities moving, and can live *next to* the world instead of inside it. You can have as many
/// as you like, each over a different key. The same structure covers three uses, which differ
/// only in where the key comes from:
///
/// - **externally assigned IDs**: the key is the outside ID (it is also a component, so the
///   reverse lookup `entity -> external ID` is an ordinary component read);
/// - **intrinsic IDs**: the key is derived from the entity's content. Use the content itself (or
///   a tuple of the relevant fields) rather than a 64-bit hash of it: a hash collision would
///   silently merge two different entities;
/// - **structural deduplication**: the same, used by
///   [`AppendWorld::insert_unique`](crate::world::AppendWorld::insert_unique) to refuse an entity
///   whose key already exists.
///
/// # Immutability
///
/// An index cannot see mutations, so the components its keys are computed from must never
/// change. That is enforced rather than left to the caller:
///
/// - only `insert_unique` can add keys, and it hands the key function a
///   [`KeyView`](crate::world::KeyView) which refuses to read any component that was not declared
///   immutable with [`AppendWorld::freeze`](crate::world::AppendWorld::freeze);
/// - the world refuses to mutate a frozen component, and freezing cannot be undone.
///
/// The index only contains the entities inserted through it. Every entity enters the world
/// through some index, but an entity inserted through one index is not in the others.
pub struct UniqueIndex<K> {
    map: HashMap<K, AppendId>,
}

impl<K> Default for UniqueIndex<K> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<K: Hash + Eq> UniqueIndex<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Looks up the entity with the given key. Accepts a borrowed form of the key, e.g. `&str`
    /// for a `String` key.
    pub fn get<Q>(&self, key: &Q) -> Option<AppendId>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.map.get(key).copied()
    }

    /// Returns the entity holding `key`, or creates one with `make` and records it. `make` is
    /// only called when the key is free, and the key is hashed only once.
    ///
    /// Not public: keys may only be added by the world, which makes sure they are derived from
    /// immutable components.
    pub(crate) fn get_or_insert_with(
        &mut self,
        key: K,
        make: impl FnOnce() -> AppendId,
    ) -> Inserted {
        match self.map.entry(key) {
            Entry::Occupied(entry) => Inserted::Existing(*entry.get()),
            Entry::Vacant(entry) => Inserted::New(*entry.insert(make())),
        }
    }
}
