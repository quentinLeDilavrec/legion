//! Implementations that let an [`EntityBuilder`] be inserted into a world.

use super::builder::EntityBuilder;
use crate::internals::{
    entity::Entity,
    insert::{ArchetypeSource, ArchetypeWriter, ComponentSource, IntoComponentSource},
    query::filter::{FilterResult, LayoutFilter},
    storage::{archetype::EntityLayout, component::ComponentTypeId},
};

impl IntoComponentSource for EntityBuilder {
    type Source = EntityBuilder;

    fn into(self) -> Self::Source {
        self
    }
}

/// A layout filter used to select the appropriate archetype for inserting
/// entities from a component source into a world.
pub struct ComponentSourceFilter(Vec<ComponentTypeId>);

impl LayoutFilter for ComponentSourceFilter {
    fn matches_layout(&self, components: &[ComponentTypeId]) -> FilterResult {
        FilterResult::Match(
            components.len() == self.0.len() && components.iter().all(|x| self.0.contains(x)),
        )
    }
}

impl ArchetypeSource for EntityBuilder {
    type Filter = ComponentSourceFilter;

    fn filter(&self) -> Self::Filter {
        let v = self.inner.info.iter().map(|x| x.0.id()).collect();
        ComponentSourceFilter(v)
    }

    fn layout(&mut self) -> EntityLayout {
        let mut layout = EntityLayout::default();

        for (tid, _offset, meta) in &self.inner.info {
            unsafe {
                layout.register_component_raw(tid.id(), *meta);
            }
        }

        layout
    }
}

impl ComponentSource for EntityBuilder {
    fn push_components(
        &mut self,
        writer: &mut ArchetypeWriter<'_>,
        mut entities: impl Iterator<Item = Entity>,
    ) {
        let entity = entities.next().unwrap();
        writer.push(entity);

        for (ty, offset, _) in &mut self.inner.info {
            let mut target = writer.claim_components_unknown(ty.id());
            let ptr = unsafe { self.inner.storage.as_ptr().add(*offset) };

            let len = 1; // actually the len of slice, not the len of the component
            unsafe { target.extend_memcopy_raw(ptr, len) };
        }

        // Ownership of the components now lies with the archetype; don't drop them again.
        self.forget_components();
    }
}
