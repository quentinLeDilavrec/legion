use std::fmt;

use legion::internals::storage::archetype::ArchetypeIndex;

/// An entity ID which *is* the entity's location: the archetype it lives in, and its position
/// inside that archetype.
///
/// Because the world is append-only, an entity never changes archetype or position, so the ID
/// never goes stale and no `ID -> location` map is needed: looking an entity up is just decoding
/// the ID. Allocating an ID is reading the archetype's current length.
///
/// The ID is `Copy` (as required by `Archetype<Id>`), 8 bytes, and unique within one world only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppendId {
    archetype: u32,
    index: u32,
}

impl AppendId {
    /// # Panics
    /// Panics if `index` does not fit in a `u32`.
    pub fn new(archetype: ArchetypeIndex, index: usize) -> Self {
        Self {
            archetype: archetype.0,
            index: u32::try_from(index).expect("too many entities in one archetype"),
        }
    }

    /// The archetype the entity lives in.
    pub fn archetype(self) -> ArchetypeIndex {
        ArchetypeIndex(self.archetype)
    }

    /// The entity's position inside its archetype, and so inside each of its component slices.
    pub fn index(self) -> usize {
        self.index as usize
    }

    /// Packs the ID into a single integer, e.g. to store it in a file or send it over a network.
    pub fn to_u64(self) -> u64 {
        (u64::from(self.archetype) << 32) | u64::from(self.index)
    }

    /// The inverse of [`to_u64`](Self::to_u64). Nothing checks that the entity exists.
    pub fn from_u64(raw: u64) -> Self {
        Self {
            archetype: (raw >> 32) as u32,
            index: raw as u32,
        }
    }
}

impl fmt::Display for AppendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.archetype, self.index)
    }
}
