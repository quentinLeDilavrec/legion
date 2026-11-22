use std::{
    any::{Any, TypeId},
    collections::HashSet,
    hash::Hash,
    mem::ManuallyDrop,
};

use legion::internals::{
    event::Subscribers,
    storage::{
        archetype::{Archetype, ArchetypeIndex, EntityLayout},
        component::{Component, ComponentTypeId},
        index::SearchIndex,
        ComponentSlice, ComponentStorage, Components,
    },
};

use crate::{
    id::AppendId,
    index::{Inserted, UniqueIndex},
};

/// A group of components inserted together as one entity. Implemented for tuples of up to six
/// components.
pub trait Bundle: Sized {
    /// Adds the bundle's component types to `layout`.
    ///
    /// # Panics
    /// Panics if the bundle contains the same component type twice.
    fn register(layout: &mut EntityLayout);

    /// Moves the bundle's components into the storages, at the end of `archetype`'s slices.
    fn write(self, components: &mut Components, archetype: ArchetypeIndex);

    /// Returns the bundle's component of type `T`, if it has one.
    fn find<T: 'static>(&self) -> Option<&T>;
}

fn write_one<T: Component>(components: &mut Components, archetype: ArchetypeIndex, value: T) {
    let storage = components
        .get_downcast_mut::<T>()
        .expect("a component's storage is created together with its archetype");
    // The storage takes ownership of the bytes, so `value` must not be dropped here.
    let value = ManuallyDrop::new(value);
    // Safety: `value` is a valid `T`, and it is never used or dropped again.
    unsafe { storage.extend_memcopy(archetype, std::ptr::from_ref::<T>(&value), 1) };
}

macro_rules! impl_bundle {
    ($($t:ident),+) => {
        impl<$($t: Component),+> Bundle for ($($t,)+) {
            fn register(layout: &mut EntityLayout) {
                $( layout.register_component::<$t>(); )+
            }

            #[allow(non_snake_case)]
            fn write(self, components: &mut Components, archetype: ArchetypeIndex) {
                let ($($t,)+) = self;
                $( write_one(components, archetype, $t); )+
            }

            #[allow(non_snake_case)]
            fn find<Wanted: 'static>(&self) -> Option<&Wanted> {
                let ($($t,)+) = self;
                $(
                    let any: &dyn Any = $t;
                    if let Some(found) = any.downcast_ref::<Wanted>() {
                        return Some(found);
                    }
                )+
                None
            }
        }
    };
}

impl_bundle!(A);
impl_bundle!(A, B);
impl_bundle!(A, B, C);
impl_bundle!(A, B, C, D);
impl_bundle!(A, B, C, D, E);
impl_bundle!(A, B, C, D, E, F);

/// A component that every entity of the world has, with a default value for entities that are
/// created without it. Type-erased so that the world can keep a list of them.
trait GlobalComponent {
    fn component_type(&self) -> ComponentTypeId;
    fn register(&self, layout: &mut EntityLayout);
    fn fill(&self, components: &mut Components, archetype: ArchetypeIndex, count: usize);
}

struct GlobalDefault<T>(T);

impl<T: Component + Clone> GlobalComponent for GlobalDefault<T> {
    fn component_type(&self) -> ComponentTypeId {
        ComponentTypeId::of::<T>()
    }

    fn register(&self, layout: &mut EntityLayout) {
        layout.register_component::<T>();
    }

    /// Appends `count` copies of the default to the end of `archetype`'s `T` slice.
    fn fill(&self, components: &mut Components, archetype: ArchetypeIndex, count: usize) {
        if count == 0 {
            return;
        }
        let storage = components
            .get_downcast_mut::<T>()
            .expect("a global component's storage exists");
        let mut values = vec![self.0.clone(); count];
        // Safety: `values` holds `count` valid elements. After the copy the storage owns them, so
        // the vector's length is zeroed to free its buffer without dropping them.
        unsafe {
            storage.extend_memcopy(archetype, values.as_ptr(), count);
            values.set_len(0);
        }
    }
}

/// Why a component could not be borrowed mutably.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutError {
    /// The entity does not exist or does not have the component.
    Missing,
    /// The component type was declared immutable with [`AppendWorld::freeze`].
    Immutable(ComponentTypeId),
}

/// What an index key function is allowed to see of the entity being inserted: its components,
/// but only those declared immutable with [`AppendWorld::freeze`].
///
/// This is what guarantees that a key never depends on data that can change later.
pub struct KeyView<'a, B> {
    bundle: &'a B,
    frozen: &'a HashSet<ComponentTypeId>,
}

impl<B: Bundle> KeyView<'_, B> {
    /// Returns the entity's component of type `T`.
    ///
    /// # Panics
    /// Panics if `T` has not been frozen, or if the entity does not have a `T`.
    pub fn get<T: Component>(&self) -> &T {
        let name = std::any::type_name::<T>();
        assert!(
            self.frozen.contains(&ComponentTypeId::of::<T>()),
            "the key reads `{name}`, which is not immutable: call `freeze::<{name}>()` first"
        );
        self.bundle
            .find::<T>()
            .unwrap_or_else(|| panic!("the key reads `{name}`, but the entity does not contain it"))
    }
}

/// A minimal append-only world.
///
/// - **Entities are never removed or moved**, so an [`AppendId`] encodes the entity's location
///   and there is no location map.
/// - **Components can still be added, to everyone, at any time** with
///   [`add_global`](Self::add_global). That changes layouts but not IDs or positions.
/// - **Components that feed an index key are immutable**, see [`freeze`](Self::freeze).
///
/// Built from the `internals` building blocks:
/// - `Archetype<AppendId>`,
/// - `Components` (with `PackedStorage` underneath), and
/// - `SearchIndex`.
#[derive(Default)]
pub struct AppendWorld {
    components: Components,
    archetypes: Vec<Archetype<AppendId>>,
    /// Mirrors `archetypes`: position `i` holds the component types of archetype `i`.
    search: SearchIndex,
    globals: Vec<Box<dyn GlobalComponent>>,
    frozen: HashSet<ComponentTypeId>,
}

impl AppendWorld {
    /// The number of entities.
    pub fn len(&self) -> usize {
        self.archetypes.iter().map(|a| a.entities().len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of archetypes. Archetypes are never removed or merged, so this only grows.
    pub fn archetype_count(&self) -> usize {
        self.archetypes.len()
    }

    pub fn contains(&self, id: AppendId) -> bool {
        self.archetypes
            .get(id.archetype().0 as usize)
            .is_some_and(|archetype| id.index() < archetype.entities().len())
    }

    /// All entity IDs, archetype by archetype.
    pub fn entities(&self) -> impl Iterator<Item = AppendId> + '_ {
        self.archetypes
            .iter()
            .flat_map(|archetype| archetype.entities().iter().copied())
    }

    /// Declares component `T` immutable, for good.
    ///
    /// From now on [`get_mut`](Self::get_mut) and [`for_each`](Self::for_each) refuse to give out
    /// mutable access to it, and index keys may be derived from it (see [`KeyView`]). Reading is
    /// unaffected, and so is inserting new entities. Freezing twice is harmless; there is
    /// deliberately no way to undo it, since that would let an index go stale.
    pub fn freeze<T: Component>(&mut self) {
        self.frozen.insert(ComponentTypeId::of::<T>());
    }

    /// Returns `true` if `T` has been frozen.
    pub fn is_frozen<T: Component>(&self) -> bool {
        self.frozen.contains(&ComponentTypeId::of::<T>())
    }

    /// Inserts an entity and returns its ID. Any global component the bundle does not contain is
    /// added with its default value.
    ///
    /// Not public: entities enter the world through [`insert_unique`](Self::insert_unique), so
    /// that every one of them has a key.
    fn push_entity<B: Bundle>(&mut self, bundle: B) -> AppendId {
        let mut layout = EntityLayout::new();
        B::register(&mut layout);

        // Globals the bundle does not provide are filled in with their default afterwards.
        let mut defaulted = Vec::new();
        for (i, global) in self.globals.iter().enumerate() {
            if !layout.has_component_by_id(global.component_type()) {
                global.register(&mut layout);
                defaulted.push(i);
            }
        }

        let archetype = self.archetype_for(layout);
        bundle.write(&mut self.components, archetype);
        for i in defaulted {
            self.globals[i].fill(&mut self.components, archetype, 1);
        }

        let slot = &mut self.archetypes[archetype];
        let id = AppendId::new(archetype, slot.entities().len());
        slot.push(id);
        id
    }

    /// Inserts an entity unless one with the same key already exists in `index`. This is the only
    /// way to add an entity to the world.
    ///
    /// Any global component (see [`add_global`](Self::add_global)) the bundle does not contain is
    /// added with its default value.
    ///
    /// `key` computes the key from a [`KeyView`] of the bundle, *before* anything is inserted, so
    /// a duplicate costs one hash lookup and the bundle is dropped. The key can read any subset
    /// of the bundle's components, which is what makes this structural deduplication, but only
    /// components declared immutable with [`freeze`](Self::freeze).
    ///
    /// # Panics
    /// Panics if `key` reads a component that is not frozen, or that the bundle does not contain.
    pub fn insert_unique<B: Bundle, K: Hash + Eq>(
        &mut self,
        index: &mut UniqueIndex<K>,
        bundle: B,
        key: impl FnOnce(KeyView<'_, B>) -> K,
    ) -> Inserted {
        let key = key(KeyView {
            bundle: &bundle,
            frozen: &self.frozen,
        });
        index.get_or_insert_with(key, || self.push_entity(bundle))
    }

    /// Adds component `T` to every entity, existing and future, without changing any ID.
    ///
    /// - Every existing entity gets a clone of `default`.
    /// - Entities inserted later get a clone of `default` too, unless their bundle contains a `T`
    ///   of its own.
    ///
    /// Each archetype keeps its index, and its entities keep their positions. Its layout is
    /// extended by rebuilding the `Archetype` (the type has no way to change a layout in place),
    /// which drops any event subscribers it had; this world has none.
    ///
    /// # Panics
    /// Panics, leaving the world untouched, if `T` is already global, or if any entity already has a `T`.
    /// Requiring that nobody has it yet is what keeps every layout unique.
    pub fn add_global<T: Component + Clone>(&mut self, default: T) {
        let type_id = ComponentTypeId::of::<T>();
        assert!(
            !self
                .globals
                .iter()
                .any(|global| global.component_type() == type_id),
            "component is already global"
        );
        assert!(
            !self
                .archetypes
                .iter()
                .any(|archetype| archetype.layout().has_component::<T>()),
            "some entities already have this component, so it cannot be added to the whole world"
        );

        let global = GlobalDefault(default);
        self.components
            .get_or_insert_with(type_id, || Box::new(T::Storage::default()));

        for i in 0..self.archetypes.len() {
            let index = self.archetypes[i].index();
            let ids = self.archetypes[i].entities().to_vec();
            let mut layout = (**self.archetypes[i].layout()).clone();
            layout.register_component::<T>();

            let mut rebuilt = Archetype::new(index, layout, Subscribers::default());
            rebuilt.reserve(ids.len());
            for id in &ids {
                rebuilt.push(*id);
            }
            self.archetypes[i] = rebuilt;

            self.components
                .get_mut(type_id)
                .expect("storage was created above")
                .insert_archetype(index, None);
            global.fill(&mut self.components, index, ids.len());
        }

        self.search = SearchIndex::default();
        for archetype in &self.archetypes {
            self.search.push(archetype.layout().component_set());
        }
        self.globals.push(Box::new(global));
    }

    /// Returns the entity's `T`, if the entity exists and has one.
    pub fn get<T: Component>(&self, id: AppendId) -> Option<&T> {
        let storage = self.components.get_downcast::<T>()?;
        storage.get(id.archetype())?.into_slice().get(id.index())
    }

    /// Returns the entity's `T` mutably.
    ///
    /// Fails with [`MutError::Immutable`] if `T` has been frozen, whether or not the entity
    /// exists, and with [`MutError::Missing`] if the entity does not exist or has no `T`.
    pub fn get_mut<T: Component>(&mut self, id: AppendId) -> Result<&mut T, MutError> {
        self.check_mutable::<T>()?;
        let storage = self
            .components
            .get_downcast::<T>()
            .ok_or(MutError::Missing)?;
        // Safety: `&mut self` gives exclusive access to every storage.
        unsafe { storage.get_mut(id.archetype()) }
            .ok_or(MutError::Missing)?
            .into_slice()
            .get_mut(id.index())
            .ok_or(MutError::Missing)
    }

    /// Iterates over every `T` in the world with its entity's ID.
    pub fn iter<T: Component>(&self) -> impl Iterator<Item = (AppendId, &T)> + '_ {
        let storage = self.components.get_downcast::<T>();
        self.archetypes.iter().flat_map(move |archetype| {
            let index = archetype.index();
            let slice = storage
                .and_then(|storage| storage.get(index))
                .map_or(&[][..], ComponentSlice::into_slice);
            slice
                .iter()
                .enumerate()
                .map(move |(i, value)| (AppendId::new(index, i), value))
        })
    }

    /// Calls `f` for every entity that has both an `A` and a `B`, reading the former and writing
    /// the latter. This is how a component added with [`add_global`](Self::add_global) gets its
    /// real values computed later.
    ///
    /// Fails with [`MutError::Immutable`], without calling `f`, if `B` has been frozen. `A` may
    /// be frozen: it is only read.
    ///
    /// # Panics
    /// Panics if `A` and `B` are the same type.
    pub fn for_each<A: Component, B: Component>(
        &mut self,
        mut f: impl FnMut(AppendId, &A, &mut B),
    ) -> Result<(), MutError> {
        assert_ne!(
            TypeId::of::<A>(),
            TypeId::of::<B>(),
            "cannot borrow the same component both shared and mutably"
        );
        self.check_mutable::<B>()?;
        let (Some(reads), Some(writes)) = (
            self.components.get_downcast::<A>(),
            self.components.get_downcast::<B>(),
        ) else {
            return Ok(());
        };

        for archetype in &self.archetypes {
            let index = archetype.index();
            // Safety: `&mut self` gives exclusive access, and `A != B` means the two storages
            // are different objects, so the shared and mutable slices cannot overlap.
            let (Some(a), Some(b)) = (reads.get(index), unsafe { writes.get_mut(index) }) else {
                continue;
            };
            for (i, (a, b)) in a.into_slice().iter().zip(b.into_slice()).enumerate() {
                f(AppendId::new(index, i), a, b);
            }
        }
        Ok(())
    }

    fn check_mutable<T: Component>(&self) -> Result<(), MutError> {
        let type_id = ComponentTypeId::of::<T>();
        if self.frozen.contains(&type_id) {
            Err(MutError::Immutable(type_id))
        } else {
            Ok(())
        }
    }

    /// Finds the archetype with exactly this layout, creating it if there is none.
    fn archetype_for(&mut self, layout: EntityLayout) -> ArchetypeIndex {
        if let Some(existing) = self.search.search(&layout).next() {
            return existing;
        }

        let index =
            ArchetypeIndex(u32::try_from(self.archetypes.len()).expect("too many archetypes"));
        self.search.push(layout.component_set());
        for (type_id, constructor) in layout
            .component_types()
            .iter()
            .zip(layout.component_constructors())
        {
            self.components
                .get_or_insert_with(*type_id, constructor)
                .insert_archetype(index, None);
        }
        self.archetypes
            .push(Archetype::new(index, layout, Subscribers::default()));
        index
    }
}
