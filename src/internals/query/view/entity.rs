#![doc(hidden)]

use super::{DefaultFilter, Fetch, IntoIndexableIter, IntoView, ReadOnly, ReadOnlyFetch, View};
use crate::internals::{
    entity::Entity,
    iter::indexed::IndexedIter,
    permissions::Permissions,
    query::{
        filter::{any::Any, passthrough::Passthrough, EntityFilterTuple},
        QueryResult,
    },
    storage::{
        archetype::{Archetype, ArchetypeIndex},
        component::{Component, ComponentTypeId},
        Components,
    },
    subworld::ComponentAccess,
};

unsafe impl ReadOnly for Entity {}

impl DefaultFilter for Entity {
    type Filter = EntityFilterTuple<Any, Passthrough>;
}

impl IntoView for Entity {
    type View = Self;
}

impl View for Entity {
    type Element<'data> = <Self::Fetch<'data> as IntoIndexableIter>::Item;
    type Fetch<'data> = EntityFetch<'data>;
    type Iter<'data, 'index> = Iter<'data, 'index>;
    type Read = [ComponentTypeId; 0];
    type Write = [ComponentTypeId; 0];

    #[inline]
    fn validate() {}

    #[inline]
    fn validate_access(_: &ComponentAccess) -> bool {
        true
    }

    #[inline]
    fn reads_types() -> Self::Read {
        []
    }

    #[inline]
    fn writes_types() -> Self::Write {
        []
    }

    #[inline]
    fn reads<D: Component>() -> bool {
        false
    }

    #[inline]
    fn writes<D: Component>() -> bool {
        false
    }

    #[inline]
    fn requires_permissions() -> Permissions<ComponentTypeId> {
        Permissions::default()
    }

    unsafe fn fetch<'data, 'index>(
        _: &'data Components,
        archetypes: &'data [Archetype],
        query: QueryResult<'index>,
    ) -> Self::Iter<'data, 'index> {
        Iter {
            archetypes,
            indexes: query.index().iter(),
        }
    }
}

#[doc(hidden)]
pub struct Iter<'a, 'i> {
    archetypes: &'a [Archetype],
    indexes: std::slice::Iter<'i, ArchetypeIndex>,
}

impl<'a, 'i> Iterator for Iter<'a, 'i> {
    type Item = Option<EntityFetch<'a>>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.indexes.next().map(|i| {
            Some(EntityFetch {
                entities: self.archetypes[*i].entities(),
            })
        })
    }
}

#[doc(hidden)]
pub struct EntityFetch<'a> {
    entities: &'a [Entity],
}

unsafe impl<'a> ReadOnlyFetch for EntityFetch<'a> {
    #[inline]
    fn get_components(&self) -> Self::Data {
        self.entities
    }
}

impl<'a> IntoIndexableIter for EntityFetch<'a> {
    type Item = &'a Entity;
    type IntoIter = IndexedIter<&'a [Entity]>;

    fn into_indexable_iter(self) -> Self::IntoIter {
        IndexedIter::new(self.entities)
    }
}

impl<'a> IntoIterator for EntityFetch<'a> {
    type Item = <Self as IntoIndexableIter>::Item;
    type IntoIter = <Self as IntoIndexableIter>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.into_indexable_iter()
    }
}

impl<'a> Fetch for EntityFetch<'a> {
    type Data = &'a [Entity];

    #[inline]
    fn into_components(self) -> Self::Data {
        self.entities
    }

    #[inline]
    fn find<C: 'static>(&self) -> Option<&[C]> {
        None
    }

    #[inline]
    fn find_mut<C: 'static>(&mut self) -> Option<&mut [C]> {
        None
    }

    #[inline]
    fn version<C: Component>(&self) -> Option<u64> {
        None
    }

    #[inline]
    fn accepted(&mut self) {}
}
