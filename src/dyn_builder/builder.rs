use std::{alloc::Layout, any::TypeId, collections::hash_map, ptr::NonNull};

use super::{type_id_map::TypeIdMap, type_info::TypeInfo};
use crate::internals::storage::{
    component::{Component, ComponentTypeId},
    UnknownComponentStorage,
};

/// Alignment of the empty (unallocated) component buffer. Must be a power of two.
const EMPTY_ALIGN: usize = 8;

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
                std::ptr::from_mut(&mut component).cast(),
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

#[cfg(feature = "internals")]
impl EntityBuilder {
    /// Adds a component
    pub fn add_with_storage<
        T: 'static + Send + Sync,
        S: 'static + Default + crate::internals::storage::UnknownComponentStorage,
    >(
        &mut self,
        mut component: T,
    ) -> &mut Self {
        // Safety: `component` is a valid instance of `T`.
        unsafe {
            self._add(
                std::ptr::from_mut(&mut component).cast(),
                TypeInfo::of::<T>(),
                || Box::new(S::default()),
            );
        }
        core::mem::forget(component);
        self
    }

    /// Returns a mutable reference to the component of type `T`, or inserts a default value if it doesn't exist.
    pub fn mut_or_default_with_storage<
        T: 'static + Default + Send + Sync,
        S: 'static + Default + crate::internals::storage::UnknownComponentStorage,
    >(
        &mut self,
    ) -> &mut T {
        self._mut_or_default(TypeInfo::of::<T>(), || Box::new(S::default()))
    }
}

#[allow(unused)] // I might use it later
impl<M> EntityBuilder<M> {
    /// Returns `true` if the entity has a component of type `T`.
    pub fn has<T: 'static>(&self) -> bool {
        self.indices.contains_key(&TypeId::of::<T>())
    }

    /// Returns a mutable reference to the component of type `T`, if it exists.
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
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

    fn _mut_or_default<T: 'static + Default>(&mut self, ty: TypeInfo, meta: M) -> &mut T {
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
                // Safety: `component` is a valid instance of `T`; ownership is moved into the
                // builder's storage by the bitwise copy, so we must not drop it here.
                let addr = unsafe {
                    let ptr = std::ptr::from_mut(&mut component).cast();
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

    /// Drops all buffered components and resets the builder.
    fn clear(&mut self) {
        for (ty, offset, _) in &self.inner.info {
            // Safety: every entry in `info` describes a live, owned component at `offset`.
            unsafe { ty.drop(self.inner.storage.as_ptr().add(*offset)) };
        }
        self.forget_components();
    }

    /// Resets the builder without dropping components. Used once ownership of the buffered
    /// components has been bitwise-moved elsewhere.
    pub(super) fn forget_components(&mut self) {
        self.inner.info.clear();
        self.inner.ids.clear();
        self.indices.clear();
        self.inner.cursor = 0;
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
    /// Allocates a new buffer of at least `min_size` bytes and copies the first `cursor` bytes of
    /// `storage` into it. Does not free `storage`.
    ///
    /// # Safety
    /// - `storage` must be valid for reads of `cursor` bytes.
    /// - `cursor <= min_size`, so the copy fits in the new buffer.
    unsafe fn grow(
        min_size: usize,
        cursor: usize,
        align: usize,
        storage: NonNull<u8>,
    ) -> (NonNull<u8>, Layout) {
        let size = min_size
            .checked_next_power_of_two()
            .expect("component buffer size overflow")
            .max(64);
        let layout = Layout::from_size_align(size, align).expect("invalid component buffer layout");
        // Safety: `layout.size()` is at least 64, so `alloc` is never called with a zero size.
        // It returns null on allocation failure, which must not be turned into a `NonNull`.
        let new_storage = NonNull::new(unsafe { std::alloc::alloc(layout) })
            .unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        // Safety: the caller guarantees `storage` is readable for `cursor` bytes. The new buffer is
        // at least `min_size >= cursor` bytes, and being freshly allocated it cannot overlap
        // `storage`. Both pointers are `u8`, so alignment is trivially satisfied. The bytes are
        // copied untyped, so padding or otherwise uninitialized bytes are fine.
        unsafe { std::ptr::copy_nonoverlapping(storage.as_ptr(), new_storage.as_ptr(), cursor) };
        (new_storage, layout)
    }

    /// Moves the bytes of a component into the buffer, growing it if needed, and returns the
    /// address the component was written to.
    ///
    /// # Safety
    /// - `ptr` must be valid for reads of `ty.layout().size()` bytes, and must not point into this buffer.
    /// - The bytes at `ptr` must be a valid instance of the type described by `ty`.
    ///   Ownership is moved into the buffer, so the caller must not drop the source afterwards.
    ///
    /// `storage` is dangling exactly when `layout.size() == 0`;
    /// otherwise it was allocated with `layout` by the global allocator.
    /// This invariant is relied on in the implementation.
    unsafe fn push(&mut self, ty: TypeInfo, ptr: *mut u8, vacant: VacantEntry, meta: M) -> *mut u8 {
        // Both steps can overflow for pathological sizes.
        // Panic rather than wrap, because a wrapped `end` would pass the capacity check below
        // and lead to out-of-bounds writes.
        let offset = self
            .cursor
            .checked_next_multiple_of(ty.layout().align())
            .expect("component buffer offset overflow");
        let end = offset
            .checked_add(ty.layout().size())
            .expect("component buffer size overflow");
        if end > self.layout.size() || ty.layout().align() > self.layout.align() {
            let new_align = self.layout.align().max(ty.layout().align());
            // Safety: `storage` holds `cursor` initialized-or-padding bytes, and
            // `end >= offset >= cursor`, since neither step above can wrap.
            let (new_storage, new_layout) =
                unsafe { Self::grow(end, self.cursor, new_align, self.storage) };
            if self.layout.size() != 0 {
                // Safety: by the struct invariant, a non-zero-size layout means `storage` was
                // allocated with exactly `self.layout`.
                // It is replaced right below, so it is never used after being freed.
                unsafe { std::alloc::dealloc(self.storage.as_ptr(), self.layout) };
            }
            self.storage = new_storage;
            self.layout = new_layout;
        }

        // Safety: the check above guarantees `end <= self.layout.size()`, so `offset` and
        // `offset + size` both lie within (or one past the end of) the allocation.
        let addr = unsafe { self.storage.as_ptr().add(offset) };
        // Safety: `addr` is valid for writes of `size` bytes (see above). `ptr` is readable for
        // `size` bytes and does not overlap the buffer, as required by this function's contract.
        unsafe { std::ptr::copy_nonoverlapping(ptr, addr, ty.layout().size()) };

        vacant.insert(self.info.len());
        self.info.push((ty, offset, meta));
        self.cursor = end;
        addr
    }
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
                // The empty buffer must be aligned to its layout's alignment: zero-sized components
                // with `align <= EMPTY_ALIGN` are "stored" at offset 0 without ever allocating, and
                // references to them must still be aligned.
                storage: NonNull::new(std::ptr::without_provenance_mut(EMPTY_ALIGN))
                    .expect("EMPTY_ALIGN is non-zero"),
                layout: Layout::from_size_align(0, EMPTY_ALIGN).unwrap(),
                cursor: 0,
                info: Vec::new(),
                ids: Vec::new(),
            },
            indices: Default::default(),
        }
    }
}
