use std::alloc::Layout;
use std::any::TypeId;
use std::collections::hash_map;
use std::ptr::NonNull;

use super::{type_id_map::TypeIdMap, type_info::TypeInfo};
use crate::internals::storage::{
    component::{Component, ComponentTypeId},
    UnknownComponentStorage,
};

/// Associated vacant entry
type VacantEntry<'a> = hash_map::VacantEntry<'a, TypeId, usize>;

/// A builder for creating entities with dynamic component storage.
pub struct EntityBuilder<M = fn() -> Box<dyn UnknownComponentStorage>> {
    pub(super) inner: Inner<M>,
    indices: TypeIdMap<usize>,
}

pub(super) struct Inner<M> {
    pub(super) storage: NonNull<u8>,
    layout: Layout,
    cursor: usize,
    pub(super) info: Vec<(TypeInfo, usize, M)>,
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

    pub(super) fn get_by_tid<T>(&self, tid: &TypeId) -> Option<&T> {
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
