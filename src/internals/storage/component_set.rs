//! An ordered set of component types.

use super::component::ComponentTypeId;
use crate::internals::query::filter::{FilterResult, LayoutFilter};

/// An ordered set of component types, without any information about how those components are stored.
///
/// Types keep the order in which they were inserted.
/// As a [`LayoutFilter`], a `ComponentSet` matches exactly the layouts containing the same types (in any order).
#[derive(Default, Debug, Clone)]
pub struct ComponentSet {
    types: Vec<ComponentTypeId>,
}

impl ComponentSet {
    /// Constructs an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a component type to the set.
    ///
    /// Returns `false`, leaving the set unchanged, if the type was already present.
    pub fn insert(&mut self, type_id: ComponentTypeId) -> bool {
        if self.types.contains(&type_id) {
            false
        } else {
            self.types.push(type_id);
            true
        }
    }

    /// Returns the component types in insertion order.
    pub fn types(&self) -> &[ComponentTypeId] {
        &self.types
    }

    /// Returns the number of component types in the set.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Returns `true` if the set contains no component types.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Returns `true` if the set contains the given component type.
    pub fn contains(&self, type_id: ComponentTypeId) -> bool {
        self.types.contains(&type_id)
    }

    /// Returns `true` if the set contains component type `T`.
    pub fn has_component<T: 'static>(&self) -> bool {
        self.contains(ComponentTypeId::of::<T>())
    }
}

impl FromIterator<ComponentTypeId> for ComponentSet {
    /// Collects component types, ignoring duplicates.
    fn from_iter<I: IntoIterator<Item = ComponentTypeId>>(iter: I) -> Self {
        let mut set = Self::new();
        for type_id in iter {
            set.insert(type_id);
        }
        set
    }
}

impl LayoutFilter for ComponentSet {
    fn matches_layout(&self, components: &[ComponentTypeId]) -> FilterResult {
        FilterResult::Match(
            components.len() == self.types.len()
                && self.types.iter().all(|t| components.contains(t)),
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn insert_rejects_duplicates_and_keeps_order() {
        let mut set = ComponentSet::new();
        assert!(set.insert(ComponentTypeId::of::<u8>()));
        assert!(set.insert(ComponentTypeId::of::<u16>()));
        assert!(!set.insert(ComponentTypeId::of::<u8>()));

        assert_eq!(set.len(), 2);
        assert_eq!(
            set.types(),
            &[ComponentTypeId::of::<u8>(), ComponentTypeId::of::<u16>()]
        );
        assert!(set.has_component::<u16>());
        assert!(!set.has_component::<u32>());
    }

    #[test]
    fn matches_the_same_types_in_any_order() {
        let set: ComponentSet = [ComponentTypeId::of::<u8>(), ComponentTypeId::of::<u16>()]
            .into_iter()
            .collect();

        let reversed = [ComponentTypeId::of::<u16>(), ComponentTypeId::of::<u8>()];
        let superset = [
            ComponentTypeId::of::<u8>(),
            ComponentTypeId::of::<u16>(),
            ComponentTypeId::of::<u32>(),
        ];
        let subset = [ComponentTypeId::of::<u8>()];

        assert!(set.matches_layout(&reversed).is_pass());
        assert!(!set.matches_layout(&superset).is_pass());
        assert!(!set.matches_layout(&subset).is_pass());
    }
}
