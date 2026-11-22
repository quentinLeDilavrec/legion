//! Archetypes are sets of entities which all contain extactly the same
//! set of component types.
//!
//! Entities in the same archetype have all of their components stored next
//! to each other and in the same order, allowing their components to be
//! accessed as zipped slices.
//!
//! Legion performs all entity filtering at the archetype level; decisions are
//! never made per-entity.

use std::{
    ops::{Index, IndexMut},
    rc::Rc,
    sync::Arc,
};

use super::{
    component::{Component, ComponentTypeId},
    component_set::ComponentSet,
    UnknownComponentStorage,
};
use crate::internals::{
    entity::Entity,
    event::{Event, Subscriber, Subscribers},
    query::filter::{FilterResult, LayoutFilter},
};

/// The index of an archetype in a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serialize", derive(serde::Serialize, serde::Deserialize))]
#[repr(transparent)]
pub struct ArchetypeIndex(pub u32);

impl<Id: Copy> Index<ArchetypeIndex> for [Archetype<Id>] {
    type Output = Archetype<Id>;

    fn index(&self, index: ArchetypeIndex) -> &Self::Output {
        &self[index.0 as usize]
    }
}

impl<Id: Copy> IndexMut<ArchetypeIndex> for [Archetype<Id>] {
    fn index_mut(&mut self, index: ArchetypeIndex) -> &mut Self::Output {
        &mut self[index.0 as usize]
    }
}

impl<Id: Copy> Index<ArchetypeIndex> for Vec<Archetype<Id>> {
    type Output = Archetype<Id>;

    fn index(&self, index: ArchetypeIndex) -> &Self::Output {
        &self[index.0 as usize]
    }
}

impl<Id: Copy> IndexMut<ArchetypeIndex> for Vec<Archetype<Id>> {
    fn index_mut(&mut self, index: ArchetypeIndex) -> &mut Self::Output {
        &mut self[index.0 as usize]
    }
}

/// An archetype is a collection of entities which all have identical component types.
///
/// `Id` is the type identifying entities,
/// which is [`Entity`] unless a custom identifier scheme is in use.
/// It must be `Copy`: events are sent by value,
/// and a bitwise copy must never apply any identifier remapping that `Clone` might perform.
#[derive(Debug)]
pub struct Archetype<Id: Copy = Entity> {
    index: ArchetypeIndex,
    entities: Vec<Id>,
    layout: Arc<EntityLayout>,
    subscribers: Subscribers<Id>,
}

impl<Id: Copy> Archetype<Id> {
    /// Constructs a new, empty archetype.
    ///
    /// - `index` is the archetype's position in whatever collection of archetypes the caller
    ///   keeps. It is reported in events and by [`index`](Self::index), and is not otherwise
    ///   interpreted.
    /// - `layout` describes the component types of the entities the archetype will hold.
    /// - `subscribers` are the listeners to notify of this archetype's events. They are sent an
    ///   `ArchetypeCreated` event immediately. Pass `Subscribers::default()` if nobody listens.
    ///
    /// The archetype only tracks entity IDs and layout; storing the entities' components is up
    /// to the component storages.
    pub fn new(
        index: ArchetypeIndex,
        layout: EntityLayout,
        mut subscribers: Subscribers<Id>,
    ) -> Self {
        subscribers.send(Event::ArchetypeCreated(index));
        Self {
            index,
            layout: Arc::new(layout),
            entities: Vec::new(),
            subscribers,
        }
    }

    /// Returns the index of the archetype.
    pub fn index(&self) -> ArchetypeIndex {
        self.index
    }

    /// Returns the layout of the archetype, describing which components are attached
    /// to all entities contained within the archetype.
    pub fn layout(&self) -> &Arc<EntityLayout> {
        &self.layout
    }

    /// Returns a slice of entity IDs for all entities which belong to the archetype.
    pub fn entities(&self) -> &[Id] {
        &self.entities
    }

    /// Appends an entity to the archetype, and notifies subscribers with an `EntityInserted`
    /// event.
    ///
    /// The entity gets the next free position, `entities().len()` before the call. The caller is
    /// responsible for appending the entity's components at the same position in every component
    /// storage of the archetype, as the archetype only tracks IDs.
    pub fn push(&mut self, entity: Id) {
        self.entities.push(entity);
        self.subscribers
            .send(Event::EntityInserted(entity, self.index));
    }

    /// Reserves capacity for at least `additional` more entities.
    ///
    /// This only affects the entity ID list; it is a performance hint and sends no events.
    pub fn reserve(&mut self, additional: usize) {
        self.entities.reserve(additional)
    }

    /// Removes the entity at `entity_index` and returns it, notifying subscribers with an
    /// `EntityRemoved` event.
    ///
    /// The last entity is moved into the vacated position, so the positions of all other entities
    /// are preserved except for the last one's. The caller must apply the same `swap_remove` to the
    /// components in every component storage of the archetype to keep them aligned.
    ///
    /// # Panics
    /// Panics if `entity_index` is out of bounds.
    pub fn swap_remove(&mut self, entity_index: usize) -> Id {
        let removed = self.entities.swap_remove(entity_index);
        self.subscribers
            .send(Event::EntityRemoved(removed, self.index));
        removed
    }

    /// Adds a subscriber to this archetype's events.
    ///
    /// The new subscriber is first brought up to date: it is sent an `ArchetypeCreated` event and
    /// an `EntityInserted` event for each entity already in the archetype. Its filter is not
    /// applied here; the caller decides which archetypes a subscriber is attached to (see
    /// `Subscriber::is_interested`).
    pub fn subscribe(&mut self, subscriber: Subscriber<Id>) {
        subscriber.send(Event::ArchetypeCreated(self.index));
        for entity in &self.entities {
            subscriber.send(Event::EntityInserted(*entity, self.index));
        }
        self.subscribers.push(subscriber);
    }

    /// Removes all entities from the archetype and returns them, in position order.
    ///
    /// Subscribers are sent an `EntityRemoved` event for each entity. The caller is responsible
    /// for removing the entities' components from every component storage of the archetype.
    pub fn drain(&mut self) -> Vec<Id> {
        let mut entities = Vec::new();
        std::mem::swap(&mut self.entities, &mut entities);
        for entity in &entities {
            self.subscribers
                .send(Event::EntityRemoved(*entity, self.index));
        }
        entities
    }
}

impl<Id: Copy> Drop for Archetype<Id> {
    fn drop(&mut self) {
        for entity in &self.entities {
            self.subscribers
                .send(Event::EntityRemoved(*entity, self.index));
        }
    }
}

/// Describes the component types which are attached to an entity.
#[derive(Default, Debug, Clone)]
pub struct EntityLayout {
    components: ComponentSet,
    component_constructors: Vec<fn() -> Box<dyn UnknownComponentStorage>>,
}

impl EntityLayout {
    /// Constructs a new entity layout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a new component to the layout.
    pub fn register_component<T: Component>(&mut self) {
        assert!(
            self.components.insert(ComponentTypeId::of::<T>()),
            "only one component of a given type may be attached to a single entity"
        );
        self.component_constructors
            .push(|| Box::new(T::Storage::default()));
    }

    /// Adds a new component to the layout.
    ///
    /// # Safety
    /// The storage returned from the storage constructor function must be capable
    /// of storing the component type identified by `type_id`.
    pub unsafe fn register_component_raw(
        &mut self,
        type_id: ComponentTypeId,
        f: fn() -> Box<dyn UnknownComponentStorage>,
    ) {
        assert!(
            self.components.insert(type_id),
            "only one component of a given type may be attached to a single entity"
        );
        self.component_constructors.push(f);
    }

    /// Returns a slice of component type IDs for the components inside the layout.
    pub fn component_types(&self) -> &[ComponentTypeId] {
        self.components.types()
    }

    /// Returns the component types of the layout as a `ComponentSet`, without the storage
    /// constructors.
    pub fn component_set(&self) -> &ComponentSet {
        &self.components
    }

    /// Returns a slice of storage constructors for each component type in the layout.
    #[doc(hidden)]
    pub fn component_constructors(&self) -> &[fn() -> Box<dyn UnknownComponentStorage>] {
        &self.component_constructors
    }

    /// Returns `true` if the layout contains the given component type.
    pub fn has_component<T: 'static>(&self) -> bool {
        self.components.has_component::<T>()
    }

    /// Returns `true` if the layout contains the given component type.
    pub fn has_component_by_id(&self, type_id: ComponentTypeId) -> bool {
        self.components.contains(type_id)
    }
}

impl LayoutFilter for EntityLayout {
    fn matches_layout(&self, components: &[ComponentTypeId]) -> FilterResult {
        self.components.matches_layout(components)
    }
}

impl LayoutFilter for Rc<EntityLayout> {
    fn matches_layout(&self, components: &[ComponentTypeId]) -> FilterResult {
        self.components.matches_layout(components)
    }
}
