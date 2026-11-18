//! Mixing hecs entity builder with IntoComponentSource to dynamically build entities.
//!
//!

use std::alloc::Layout;
use std::any::TypeId;
use std::collections::{hash_map, HashMap};
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};
use std::ptr::NonNull;

use crate::query::{FilterResult, LayoutFilter};
use crate::storage::{
    ArchetypeSource, ArchetypeWriter, Component, ComponentSource, ComponentTypeId, EntityLayout,
    IntoComponentSource, UnknownComponentStorage,
};
use crate::Entity;

/// A hasher optimized for hashing a single TypeId.
///
/// TypeId is already thoroughly hashed, so there's no reason to hash it again.
/// Just leave the bits unchanged.
#[derive(Default)]
pub(crate) struct TypeIdHasher {
    hash: u64,
}

impl Hasher for TypeIdHasher {
    fn write_u64(&mut self, n: u64) {
        // Only a single value can be hashed, so the old hash should be zero.
        debug_assert_eq!(self.hash, 0);
        self.hash = n;
    }

    // Tolerate TypeId being either u64 or u128.
    fn write_u128(&mut self, n: u128) {
        debug_assert_eq!(self.hash, 0);
        self.hash = n as u64;
    }

    fn write(&mut self, bytes: &[u8]) {
        debug_assert_eq!(self.hash, 0);

        // This will only be called if TypeId is neither u64 nor u128, which is not anticipated.
        // In that case we'll just fall back to using a different hash implementation.
        let mut hasher = <BuildHasherDefault<DefaultHasher> as BuildHasher>::Hasher::default();
        hasher.write(bytes);
        self.hash = hasher.finish();
    }

    fn finish(&self) -> u64 {
        self.hash
    }
}

/// A HashMap with TypeId keys
///
/// Because TypeId is already a fully-hashed u64 (including data in the high seven bits,
/// which hashbrown needs), there is no need to hash it again. Instead, this uses the much
/// faster no-op hash.
pub(crate) type TypeIdMap<V> = HashMap<TypeId, V, core::hash::BuildHasherDefault<TypeIdHasher>>;

/// Associated vacant entry
type VacantEntry<'a> = hash_map::VacantEntry<'a, TypeId, usize>;
/// Metadata required to store a component.
///
/// All told, this means a [`TypeId`], to be able to dynamically name/check the component type; a
/// [`Layout`], so that we know how to allocate memory for this component type; and a drop function
/// which internally calls [`core::ptr::drop_in_place`] with the correct type parameter.
#[derive(Debug, Copy, Clone)]
pub struct TypeInfo {
    id: ComponentTypeId,
    layout: Layout,
    drop: unsafe fn(*mut u8),
    // #[cfg(debug_assertions)]
    // type_name: &'static str,
}

impl TypeInfo {
    /// Construct a `TypeInfo` directly from the static type.
    pub fn of<T: 'static + Send + Sync>() -> Self {
        unsafe fn drop_ptr<T>(x: *mut u8) {
            unsafe { x.cast::<T>().drop_in_place() }
        }

        Self {
            id: ComponentTypeId::of::<T>(),
            layout: Layout::new::<T>(),
            drop: drop_ptr::<T>,
            // #[cfg(debug_assertions)]
            // type_name: core::any::type_name::<T>(),
        }
    }

    // /// Construct a `TypeInfo` from its components. This is useful in the rare case that you have
    // /// some kind of pointer to raw bytes/erased memory holding a component type, coming from a
    // /// source unrelated to hecs, and you want to treat it as an insertable component by
    // /// implementing the `DynamicBundle` API.
    // pub fn from_parts(id: ComponentTypeId, layout: Layout, drop: unsafe fn(*mut u8)) -> Self {
    //     Self {
    //         id,
    //         layout,
    //         drop,
    //         // #[cfg(debug_assertions)]
    //         // type_name: "<unknown> (TypeInfo constructed from parts)",
    //     }
    // }

    /// Access the `TypeId` for this component type.
    pub fn id(&self) -> ComponentTypeId {
        self.id
    }

    /// Access the `Layout` of this component type.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Directly call the destructor on a pointer to data of this component type.
    ///
    /// # Safety
    ///
    /// All of the caveats of [`core::ptr::drop_in_place`] apply, with the additional requirement
    /// that this method is being called on a pointer to an object of the correct component type.
    pub unsafe fn drop(&self, data: *mut u8) {
        unsafe { (self.drop)(data) }
    }

    /// Get the function pointer encoding the destructor for the component type this `TypeInfo`
    /// represents.
    pub fn drop_shim(&self) -> unsafe fn(*mut u8) {
        self.drop
    }
}

/// A builder for creating entities with dynamic component storage.
pub struct EntityBuilder<M = fn() -> Box<dyn UnknownComponentStorage>> {
    inner: Inner<M>,
    indices: TypeIdMap<usize>,
}

struct Inner<M> {
    storage: NonNull<u8>,
    layout: Layout,
    cursor: usize,
    info: Vec<(TypeInfo, usize, M)>,
    ids: Vec<TypeId>,
}

impl EntityBuilder {
    /// Adds a component
    pub fn add<T: Component>(&mut self, mut component: T) -> &mut Self {
        // Safety: `component` is a valid instance of `T`.
        unsafe {
            self._add(
                (&mut component as *mut T).cast(),
                TypeInfo::of::<T>(),
                || Box::new(T::Storage::default()),
            );
        }
        core::mem::forget(component);
        self
    }

    /// Returns a mutable reference to the component of type `T`, or inserts a default value if it doesn't exist.
    pub fn mut_or_default<T: Component + Default>(&mut self) -> &mut T {
        self._mut_or_default(TypeInfo::of::<T>(), || Box::new(T::Storage::default()))
    }
}

#[allow(unused)] // I might use it later
impl<M> EntityBuilder<M> {
    /// Returns `true` if the entity has a component of type `T`.
    pub fn has<T: Component>(&self) -> bool {
        self.indices.contains_key(&TypeId::of::<T>())
    }

    /// Returns a mutable reference to the component of type `T`, if it exists.
    pub fn get_mut<T: Component>(&mut self) -> Option<&mut T> {
        let index = self.indices.get(&TypeId::of::<T>())?;
        let (_, offset, _) = self.inner.info[*index];
        unsafe {
            let storage = (self.inner.storage.as_ptr()).add(offset).cast::<T>();
            Some(&mut *storage)
        }
    }

    fn get_by_tid<T>(&self, tid: &TypeId) -> Option<&T> {
        let index = self.indices.get(tid)?;
        let (_, offset, _) = self.inner.info[*index];
        unsafe {
            let storage = self.inner.storage.as_ptr().add(offset).cast::<T>();
            Some(&*storage)
        }
    }

    fn _mut_or_default<T: Component + Default>(&mut self, ty: TypeInfo, meta: M) -> &mut T {
        use hash_map::Entry;
        match self.indices.entry(ty.id().type_id()) {
            Entry::Occupied(occupied) => {
                let index = occupied.get();
                let (_, offset, _) = self.inner.info[*index];
                unsafe {
                    let storage = (self.inner.storage.as_ptr()).add(offset).cast::<T>();
                    &mut *storage
                }
            }
            Entry::Vacant(vacant) => {
                let mut component = T::default();
                let addr = unsafe {
                    let ptr = (&mut T::default() as *mut T).cast();
                    self.inner.push(ty, ptr, vacant, meta)
                };
                core::mem::forget(component);
                unsafe {
                    let storage = addr.cast::<T>();
                    &mut *storage
                }
            }
        }
    }

    fn component_types(&self) -> impl Iterator<Item = ComponentTypeId> + '_ {
        self.inner.info.iter().map(|(info, _, _)| info.id())
    }

    fn clear(&mut self) {
        self.inner.ids.clear();
        self.indices.clear();
        self.inner.cursor = 0;
        // NOTE we do not clone stuff and use everything, thus we do not need to drop things pointed by structures in storage
        // unsafe {
        //     for (ty, offset, _) in self.info.drain(..) {
        //         ty.drop(self.storage.as_ptr().add(offset));
        //     }
        // }
    }

    unsafe fn _add(&mut self, ptr: *mut u8, ty: TypeInfo, meta: M) {
        use hash_map::Entry;
        match self.indices.entry(ty.id().type_id()) {
            Entry::Occupied(occupied) => {
                let index = *occupied.get();
                let (ty, offset, _) = self.inner.info[index];
                unsafe {
                    let storage = self.inner.storage.as_ptr().add(offset);

                    // Drop the existing value
                    ty.drop(storage);

                    // Overwrite the old value with our new one.
                    std::ptr::copy_nonoverlapping(ptr, storage, ty.layout().size());
                }
            }
            Entry::Vacant(vacant) => {
                unsafe { self.inner.push(ty, ptr, vacant, meta) };
            }
        }
    }
}

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
    }
}

impl<M> Inner<M> {
    unsafe fn grow(
        min_size: usize,
        cursor: usize,
        align: usize,
        storage: NonNull<u8>,
    ) -> (NonNull<u8>, Layout) {
        let layout = Layout::from_size_align(min_size.next_power_of_two().max(64), align).unwrap();
        let new_storage = unsafe { NonNull::new_unchecked(std::alloc::alloc(layout)) };
        unsafe { std::ptr::copy_nonoverlapping(storage.as_ptr(), new_storage.as_ptr(), cursor) };
        (new_storage, layout)
    }

    unsafe fn push(&mut self, ty: TypeInfo, ptr: *mut u8, vacant: VacantEntry, meta: M) -> *mut u8 {
        let offset = align(self.cursor, ty.layout().align());
        let end = offset + ty.layout().size();
        if end > self.layout.size() || ty.layout().align() > self.layout.align() {
            let new_align = self.layout.align().max(ty.layout().align());
            let (new_storage, new_layout) =
                unsafe { Self::grow(end, self.cursor, new_align, self.storage) };
            if self.layout.size() != 0 {
                unsafe { std::alloc::dealloc(self.storage.as_ptr(), self.layout) };
            }
            self.storage = new_storage;
            self.layout = new_layout;
        }

        let addr = unsafe { self.storage.as_ptr().add(offset) };
        unsafe { std::ptr::copy_nonoverlapping(ptr, addr, ty.layout().size()) };

        vacant.insert(self.info.len());
        self.info.push((ty, offset, meta));
        self.cursor = end;
        addr
    }
}

fn align(x: usize, alignment: usize) -> usize {
    debug_assert!(alignment.is_power_of_two());
    (x + alignment - 1) & (!alignment + 1)
}

unsafe impl<M: Send> Send for EntityBuilder<M> {}
unsafe impl<M: Sync> Sync for EntityBuilder<M> {}

impl<M> Drop for EntityBuilder<M> {
    fn drop(&mut self) {
        // Ensure buffered components aren't leaked
        self.clear();
        if self.inner.layout.size() != 0 {
            unsafe {
                std::alloc::dealloc(self.inner.storage.as_ptr(), self.inner.layout);
            }
        }
    }
}

impl<M> Default for EntityBuilder<M> {
    /// Create a builder representing an entity with no components
    fn default() -> Self {
        Self {
            inner: Inner {
                storage: NonNull::dangling(),
                layout: Layout::from_size_align(0, 8).unwrap(),
                cursor: 0,
                info: Vec::new(),
                ids: Vec::new(),
            },
            indices: Default::default(),
        }
    }
}

#[test]
fn simple() {
    let mut world = crate::World::new(Default::default());
    let mut components = EntityBuilder::default();
    components.add(42i32);
    components.add(true);
    components.add(vec![0, 1, 2, 3]);
    components.add("hello");
    components.add(0u64);
    let entity = world.extend(components)[0];
    assert_eq!(Ok(&42), world.entry(entity).unwrap().get_component::<i32>());
    assert_eq!(
        Ok(&vec![0, 1, 2, 3]),
        world.entry(entity).unwrap().get_component::<Vec<i32>>()
    );
}

#[test]
fn test_cloning() {
    let mut world = crate::World::new(Default::default());
    let mut components = EntityBuilder::default();
    let mut comp0: (Box<[u32]>,) = (vec![0, 0, 0, 0, 0, 1, 4100177920].into_boxed_slice(),); //0, 14, 43, 10, 876, 7, 1065, 35
    let mut comp0_saved = comp0.clone();
    let comp0_ptr = (&mut comp0) as *mut (Box<[u32]>,);
    components.add(comp0);
    unsafe { (*comp0_ptr).0[4] = 42 };
    comp0_saved.0[4] = 42;
    let comp1: i32 = 0;
    components.add(comp1);
    let comp2: bool = true;
    components.add(comp2);
    let mut comp3: Vec<u64> = vec![0, 1, 2, 3];
    let comp3_saved = comp3.clone();
    let comp3_ptr = (&mut comp3) as *mut Vec<u64>;
    components.add(comp3);
    let comp4: String = "ewgwgwsegwesf".into();
    components.add(comp4.clone());
    let comp5: u64 = 0;
    components.add(comp5);
    dbg!(unsafe { comp0_ptr.as_ref() });
    let entity = world.extend(components)[0];
    assert_eq!(
        Some(&comp0_saved),
        unsafe { comp0_ptr.as_ref() },
        "slice should not have changed"
    );
    assert_eq!(
        Some(&comp3_saved),
        unsafe { comp3_ptr.as_ref() },
        "vec should not have changed"
    );

    if let Some(entry) = world.entry(entity) {
        unsafe { (*comp0_ptr).0[5] += 1 };
        comp0_saved.0[5] += 1;
        dbg!(unsafe { comp0_ptr.as_ref() });
        assert_eq!(Ok(&comp0_saved), entry.get_component::<(Box<[u32]>,)>());
        assert_eq!(Ok(&comp1), entry.get_component::<i32>());
        assert_eq!(Ok(&comp2), entry.get_component::<bool>());
        assert_eq!(Ok(&comp3_saved), entry.get_component::<Vec<u64>>());
        assert_eq!(Ok(&comp4), entry.get_component::<String>());
    }
}

#[test]
fn test_get_mut() {
    let mut components = EntityBuilder::default();
    components.add(42i32);
    components.add(true);
    components.add(vec![0, 1, 2, 3]);
    components.add("hello");
    components.add(0u64);

    assert_eq!(components.get_mut::<i32>(), Some(&mut 42));
    assert_eq!(components.get_mut::<bool>(), Some(&mut true));
    assert_eq!(
        components.get_mut::<Vec<i32>>(),
        Some(&mut vec![0, 1, 2, 3])
    );
    assert_eq!(components.get_mut::<&'static str>(), Some(&mut "hello"));
    assert_eq!(components.get_mut::<u64>(), Some(&mut 0));
    assert_eq!(components.get_mut::<()>(), None);
    components.get_mut::<Vec<i32>>().unwrap()[0] += 1;

    assert_eq!(
        components.get_mut::<Vec<i32>>(),
        Some(&mut vec![1, 1, 2, 3])
    );
}

#[test]
fn test_mut_or_default() {
    let mut components = EntityBuilder::default();
    *components.mut_or_default::<i32>() += 1;
    assert_eq!(*components.mut_or_default::<i32>(), 1);
}

#[test]
fn test_send() {
    fn assert_send<T: Send>() {}
    assert_send::<EntityBuilder>();

    // Fill the builder on this thread, including heap-owning components.
    let mut components = EntityBuilder::default();
    components.add(vec![1, 2, 3]);
    components.add(String::from("hello"));

    // Move it to another thread, read and mutate it there, then move it back.
    let Ok(mut components) = std::thread::spawn(move || {
        assert_eq!(components.get_mut::<Vec<i32>>(), Some(&mut vec![1, 2, 3]));
        assert_eq!(
            components.get_mut::<String>().map(|s| s.as_str()),
            Some("hello")
        );
        *components.mut_or_default::<i32>() += 1;
        components.get_mut::<Vec<i32>>().unwrap().push(4);
        components
    })
    .join() else {
        panic!();
    };

    // The data written on both threads is still intact.
    assert_eq!(*components.mut_or_default::<i32>(), 1);
    assert_eq!(
        components.get_mut::<Vec<i32>>(),
        Some(&mut vec![1, 2, 3, 4])
    );
    assert_eq!(
        components.get_mut::<String>().map(|s| s.as_str()),
        Some("hello")
    );
}

#[test]
fn test_sync() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<EntityBuilder>();

    let mut components = EntityBuilder::default();
    *components.mut_or_default::<i32>() += 1;
    components.add(true);

    // Share `&EntityBuilder` between several threads at the same time.
    let components = &components;
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                s.spawn(move || {
                    assert!(components.has::<i32>());
                    assert!(components.has::<bool>());
                    assert!(!components.has::<u64>());
                    assert_eq!(components.get_by_tid::<i32>(&TypeId::of::<i32>()), Some(&1));
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("reader thread panicked");
        }
    });
}
