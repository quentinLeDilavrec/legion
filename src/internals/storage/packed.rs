//! Component storage which can pack archetypes into contiguous memory.

use std::{
    alloc::Layout,
    cell::UnsafeCell,
    iter::Zip,
    mem::size_of,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    rc::Rc,
    slice::Iter,
};

use super::{
    archetype::ArchetypeIndex, component::Component, next_component_version, ComponentIndex,
    ComponentMeta, ComponentSlice, ComponentSliceMut, ComponentStorage, Epoch,
    UnknownComponentStorage,
};

/// A memory allocation for an array of `T`.
#[derive(Debug)]
struct RawAlloc<T> {
    ptr: NonNull<T>,
    cap: usize,
}

impl<T> RawAlloc<T> {
    fn new(min_capacity: usize) -> Self {
        if size_of::<T>() == 0 {
            Self {
                ptr: NonNull::dangling(),
                cap: !0,
            }
        } else if min_capacity == 0 {
            Self {
                ptr: NonNull::dangling(),
                cap: 0,
            }
        } else {
            let layout = Self::array_layout(min_capacity);
            // Safety: `T` is not zero-sized and `min_capacity` is non-zero, so the size is non-zero.
            let ptr = NonNull::new(unsafe { std::alloc::alloc(layout) })
                .unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
            Self {
                ptr: ptr.cast(),
                cap: min_capacity,
            }
        }
    }

    fn grow(&mut self, new_capacity: usize) {
        debug_assert!(size_of::<T>() != 0);
        debug_assert!(self.cap < new_capacity);

        let new_layout = Self::array_layout(new_capacity);
        let dst_ptr = if self.cap == 0 {
            // Safety: `T` is not zero-sized and `new_capacity > cap >= 0`, so the size is non-zero.
            unsafe { std::alloc::alloc(new_layout) }
        } else {
            // Safety: `ptr` is a live allocation made by the global allocator with `self.layout()`.
            // `new_layout.size()` is non-zero, and, being the size of a valid `Layout` with the same
            // alignment, does not overflow `isize::MAX` when rounded up to that alignment.
            unsafe {
                std::alloc::realloc(
                    self.ptr.as_ptr().cast::<u8>(),
                    self.layout(),
                    new_layout.size(),
                )
            }
        };
        match NonNull::new(dst_ptr) {
            Some(new_ptr) => {
                self.ptr = new_ptr.cast();
                self.cap = new_capacity;
            }
            None => std::alloc::handle_alloc_error(new_layout),
        }
    }

    /// The layout of an array of `capacity` elements.
    ///
    /// Panics if the total size overflows `usize` or exceeds `isize::MAX`.
    fn array_layout(capacity: usize) -> Layout {
        Layout::array::<T>(capacity).expect("component storage capacity overflow")
    }

    /// The layout of the current heap allocation.
    ///
    /// Must only be called while the allocation exists, i.e. `T` is not zero-sized and `cap > 0`.
    fn layout(&self) -> Layout {
        debug_assert!(size_of::<T>() != 0 && self.cap > 0);
        // Safety: `cap` was only ever set after `Layout::array::<T>(cap)` succeeded when the
        // allocation was made or grown, so it succeeds again here.
        unsafe { Layout::array::<T>(self.cap).unwrap_unchecked() }
    }
}

impl<T> Drop for RawAlloc<T> {
    fn drop(&mut self) {
        if size_of::<T>() != 0 && self.cap > 0 {
            // Safety: a non-zero-sized `T` with `cap > 0` means `ptr` is a live allocation made by
            // the global allocator with exactly `self.layout()`.
            unsafe { std::alloc::dealloc(self.ptr.as_ptr().cast(), self.layout()) };
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)] // it isn't dead - apparent rustc bug
enum ComponentVec<T> {
    Packed {
        raw: Rc<RawAlloc<T>>,
        offset: usize,
        len: usize,
        cap: usize,
    },
    Loose {
        raw: RawAlloc<T>,
        len: usize,
        last_written: Epoch,
    },
}

impl<T> ComponentVec<T> {
    fn new() -> Self {
        Self::Loose {
            raw: RawAlloc::new(0),
            len: 0,
            last_written: 0,
        }
    }

    fn should_pack(&self, epoch_threshold: Epoch) -> bool {
        match self {
            Self::Loose { last_written, .. } => *last_written <= epoch_threshold,
            _ => true,
        }
    }

    fn as_raw_slice(&self) -> (NonNull<T>, usize) {
        match self {
            Self::Packed {
                raw, offset, len, ..
            } => {
                // Safety: a packed slice always lies within the shared allocation,
                // i.e. `offset + len <= raw.cap` (established in `ComponentVec::pack`),
                // so `add` stays in bounds.
                (unsafe { raw.ptr.add(*offset) }, *len)
            }
            Self::Loose { raw, len, .. } => (raw.ptr, *len),
        }
    }

    fn estimate_fragmentation(&self) -> f32 {
        match self {
            Self::Loose { .. } => 1f32,
            Self::Packed { len, cap, .. } => {
                let empty = cap - len;
                f32::min(1f32, empty as f32 * size_of::<T>() as f32 / 16f32)
            }
        }
    }

    unsafe fn extend_memcopy(&mut self, epoch: Epoch, ptr: *const T, count: usize) {
        self.ensure_capacity(epoch, count);
        let (dst, len) = self.as_raw_slice();
        unsafe { std::ptr::copy_nonoverlapping(ptr, dst.as_ptr().add(len), count) };
        match self {
            Self::Packed { len, .. } => *len += count,
            Self::Loose {
                len, last_written, ..
            } => {
                *len += count;
                *last_written = epoch;
            }
        }
    }

    fn ensure_capacity(&mut self, epoch: Epoch, space: usize) {
        let (cap, len) = match self {
            Self::Packed { cap, len, .. } => (*cap, *len),
            Self::Loose { raw, len, .. } => (raw.cap, *len),
        };

        if cap - len < space {
            let new_capacity = len
                .checked_mul(2)
                .and_then(|doubled| doubled.checked_add(space))
                .expect("component storage capacity overflow");
            self.grow(epoch, new_capacity);
        }
    }

    fn swap_remove(&mut self, epoch: Epoch, index: usize) -> T {
        let (ptr, len) = self.as_raw_slice();
        assert!(len > index);

        unsafe {
            let item_ptr = ptr.as_ptr().add(index);
            let last_ptr = ptr.as_ptr().add(len - 1);
            if index < len - 1 {
                std::ptr::swap(item_ptr, last_ptr);
            }
            let value = std::ptr::read(last_ptr);
            match self {
                Self::Packed { len, .. } => *len -= 1,
                Self::Loose {
                    len, last_written, ..
                } => {
                    *len -= 1;
                    *last_written = epoch;
                }
            }
            value
        }
    }

    fn grow(&mut self, epoch: Epoch, new_capacity: usize) {
        debug_assert_ne!(std::mem::size_of::<T>(), 0);

        match self {
            Self::Packed {
                raw,
                offset,
                len,
                cap,
            } => {
                // if we are currently packed, then allocate new storage and switch to loose
                debug_assert!(*cap < new_capacity);
                let new_alloc = RawAlloc::new(new_capacity);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        raw.ptr.as_ptr().add(*offset),
                        new_alloc.ptr.as_ptr(),
                        *len,
                    )
                };
                *self = Self::Loose {
                    raw: new_alloc,
                    len: *len,
                    last_written: epoch,
                };
            }
            Self::Loose {
                raw, last_written, ..
            } => {
                // if we are already free, try and resize the allocation
                raw.grow(new_capacity);
                *last_written = epoch;
            }
        };
    }

    /// Forgets all elements without dropping them, leaving the vec empty but still owning its
    /// buffer.
    ///
    /// # Safety
    /// The caller must have moved the elements out (e.g. by memcopy), so that dropping this vec
    /// does not double-drop them.
    unsafe fn forget_elements(&mut self) {
        match self {
            Self::Packed { len, .. } | Self::Loose { len, .. } => *len = 0,
        }
    }

    unsafe fn pack(&mut self, dst: Rc<RawAlloc<T>>, offset: usize) {
        let (ptr, len) = self.as_raw_slice();
        debug_assert_ne!(std::mem::size_of::<T>(), 0);
        debug_assert!(dst.cap >= offset.checked_add(len).unwrap());
        unsafe { std::ptr::copy_nonoverlapping(ptr.as_ptr(), dst.ptr.as_ptr().add(offset), len) };
        *self = Self::Packed {
            raw: dst,
            offset,
            len,
            cap: len,
        }
    }
}

impl<T> Deref for ComponentVec<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        let (ptr, len) = self.as_raw_slice();
        unsafe { std::slice::from_raw_parts(ptr.as_ptr(), len) }
    }
}

impl<T> DerefMut for ComponentVec<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let (ptr, len) = self.as_raw_slice();
        unsafe { std::slice::from_raw_parts_mut(ptr.as_ptr(), len) }
    }
}

impl<T> Drop for ComponentVec<T> {
    fn drop(&mut self) {
        if std::mem::needs_drop::<T>() {
            unsafe {
                let (ptr, len) = self.as_raw_slice();
                for i in 0..len {
                    std::ptr::drop_in_place(ptr.as_ptr().add(i));
                }
            }
        }
    }
}

/// Stores a slice of components of type `T` for each archetype.
/// Archetype slices are sorted according to the group that component `T` belongs to.
/// Each slice _may_ be packed into a single allocation to optimise for group-based access.
#[derive(Debug)]
pub struct PackedStorage<T: Component> {
    // Sparse indirection table
    index: Vec<usize>,
    // Ordered archetype slices: (ptr, len)
    slices: Vec<(NonNull<T>, usize)>,
    // The total number of components stored
    entity_len: usize,
    // The current epoch
    epoch: Epoch,
    // Ordered archetype versions
    versions: Vec<UnsafeCell<u64>>,
    // Ordered allocation metadata
    allocations: Vec<ComponentVec<T>>,
}

// these are needed because of the UnsafeCell in versions
// but we write protect that ourselves
unsafe impl<T: Component> Send for PackedStorage<T> {}
unsafe impl<T: Component> Sync for PackedStorage<T> {}

impl<T: Component> PackedStorage<T> {
    fn swap_remove_internal(
        &mut self,
        ArchetypeIndex(archetype): ArchetypeIndex,
        ComponentIndex(index): ComponentIndex,
    ) -> T {
        let slice_index = self.index[archetype as usize];
        let allocation = &mut self.allocations[slice_index];
        let component = allocation.swap_remove(self.epoch, index);
        self.update_slice(slice_index);
        self.entity_len -= 1;
        component
    }

    #[inline]
    fn update_slice(&mut self, slice_index: usize) {
        self.slices[slice_index] = self.allocations[slice_index].as_raw_slice();
    }

    fn index(&self, ArchetypeIndex(archetype): ArchetypeIndex) -> usize {
        self.index[archetype as usize]
    }
}

impl<T: Component> UnknownComponentStorage for PackedStorage<T> {
    fn move_component(
        &mut self,
        source: ArchetypeIndex,
        index: ComponentIndex,
        dst: ArchetypeIndex,
    ) {
        // find archetype locations
        let src_slice_index = self.index(source);
        let dst_slice_index = self.index(dst);

        // remove component from source slice
        let src_allocation = &mut self.allocations[src_slice_index];
        let value = src_allocation.swap_remove(self.epoch, index.0);

        // insert component into destination slice
        let dst_allocation = &mut self.allocations[dst_slice_index];
        unsafe {
            dst_allocation.extend_memcopy(self.epoch, std::ptr::from_ref(&value), 1);
            *self.versions[dst_slice_index].get() = next_component_version();
        }

        // update slice pointers
        self.update_slice(src_slice_index);
        self.update_slice(dst_slice_index);

        // forget component to prevent it being dropped (we copied it into the destination)
        std::mem::forget(value);
    }

    fn insert_archetype(&mut self, archetype: ArchetypeIndex, index: Option<usize>) {
        let index = index.unwrap_or(self.slices.len());
        let arch_index = archetype.0 as usize;

        // create new vector for archetype
        let allocation = ComponentVec::<T>::new();

        // insert archetype into collections
        self.slices.insert(index, allocation.as_raw_slice());
        self.versions.insert(index, UnsafeCell::new(0));
        self.allocations.insert(index, allocation);

        // update index
        for i in self.index.iter_mut().filter(|i| **i != !0 && **i >= index) {
            *i += 1;
        }
        if arch_index >= self.index.len() {
            self.index.resize(arch_index + 1, !0);
        }
        self.index[arch_index] = index;
    }

    fn transfer_archetype(
        &mut self,
        src_archetype: ArchetypeIndex,
        dst_archetype: ArchetypeIndex,
        dst: &mut dyn UnknownComponentStorage,
    ) {
        let dst = dst.downcast_mut::<Self>().unwrap();
        let src_index = self.index(src_archetype);
        let dst_index = dst.index(dst_archetype);

        // update total counts
        let count = self.allocations[src_index].len();
        self.entity_len -= count;
        dst.entity_len += count;

        if dst.allocations[dst_index].is_empty() {
            // fast path:
            // swap the allocations
            std::mem::swap(
                &mut self.allocations[src_index],
                &mut dst.allocations[dst_index],
            );

            // bump destination version
            unsafe { *dst.versions[dst_index].get() = next_component_version() };
        } else {
            // memcopy components into the destination
            let (ptr, len) = self.get_raw(src_archetype).unwrap();
            unsafe { dst.extend_memcopy_raw(dst_archetype, ptr, len) };

            // The components were memcopied into the destination, so forget them in the source
            // (without running their destructors) and then drop the now-empty vec so that its
            // buffer, or its reference to a shared packed buffer, is released.
            let src_allocation = &mut self.allocations[src_index];
            unsafe { src_allocation.forget_elements() };
            *src_allocation = ComponentVec::new();
        }

        // update slice pointers
        self.update_slice(src_index);
        dst.update_slice(dst_index);
    }

    /// Moves a component to a new storage.
    fn transfer_component(
        &mut self,
        src_archetype: ArchetypeIndex,
        src_component: ComponentIndex,
        dst_archetype: ArchetypeIndex,
        dst: &mut dyn UnknownComponentStorage,
    ) {
        let component = self.swap_remove_internal(src_archetype, src_component);
        unsafe {
            dst.extend_memcopy_raw(
                dst_archetype,
                std::ptr::from_ref(&component).cast::<u8>(),
                1,
            )
        };
        std::mem::forget(component);
    }

    fn swap_remove(&mut self, archetype: ArchetypeIndex, index: ComponentIndex) {
        self.swap_remove_internal(archetype, index);
    }

    fn pack(&mut self, age_threshold: Epoch) -> usize {
        if size_of::<T>() == 0 {
            return 0;
        }

        let epoch_threshold = self.epoch.saturating_sub(age_threshold);

        let len = self
            .slices
            .iter()
            .zip(self.allocations.iter())
            .filter(|(_, allocation)| allocation.should_pack(epoch_threshold))
            .map(|((_, len), _)| *len)
            .sum();

        unsafe {
            let packed = Rc::new(RawAlloc::new(len));

            let mut cursor = 0;
            for (alloc, slice) in self
                .allocations
                .iter_mut()
                .zip(self.slices.iter_mut())
                .filter(|(allocation, _)| allocation.should_pack(epoch_threshold))
            {
                alloc.pack(Rc::clone(&packed), cursor);
                *slice = alloc.as_raw_slice();
                cursor += slice.1;
            }

            cursor
        }
    }

    fn fragmentation(&self) -> f32 {
        if self.entity_len == 0 {
            return 0f32;
        }

        self.allocations
            .iter()
            .fold(0f32, |x, y| x + y.estimate_fragmentation())
            / self.entity_len as f32
    }

    fn element_vtable(&self) -> ComponentMeta {
        ComponentMeta::of::<T>()
    }

    fn get_raw(&self, ArchetypeIndex(archetype): ArchetypeIndex) -> Option<(*const u8, usize)> {
        let slice_index = *self.index.get(archetype as usize)?;
        let (ptr, len) = self.slices.get(slice_index)?;
        Some((ptr.as_ptr() as *const u8, *len))
    }

    unsafe fn get_mut_raw(
        &self,
        ArchetypeIndex(archetype): ArchetypeIndex,
    ) -> Option<(*mut u8, usize)> {
        let slice_index = *self.index.get(archetype as usize)?;
        let (ptr, len) = self.slices.get(slice_index)?;
        unsafe { *self.versions.get_unchecked(slice_index).get() = next_component_version() };
        Some((ptr.as_ptr().cast::<u8>(), *len))
    }

    unsafe fn extend_memcopy_raw(
        &mut self,
        ArchetypeIndex(archetype): ArchetypeIndex,
        ptr: *const u8,
        count: usize,
    ) {
        let slice_index = self.index[archetype as usize];
        let allocation = &mut self.allocations[slice_index];
        unsafe { allocation.extend_memcopy(self.epoch, ptr.cast::<T>(), count) };
        self.slices[slice_index] = allocation.as_raw_slice();
        self.entity_len += count;
        unsafe { *self.versions[slice_index].get() = next_component_version() };
    }

    fn increment_epoch(&mut self) {
        self.epoch += 1;
    }

    fn ensure_capacity(&mut self, ArchetypeIndex(archetype): ArchetypeIndex, capacity: usize) {
        let slice_index = self.index[archetype as usize];
        let allocation = &mut self.allocations[slice_index];
        allocation.ensure_capacity(self.epoch, capacity);
    }
}

impl<T: Component> Default for PackedStorage<T> {
    fn default() -> Self {
        Self {
            index: Vec::new(),
            slices: Vec::new(),
            versions: Vec::new(),
            allocations: Vec::new(),
            entity_len: 0,
            epoch: 0,
        }
    }
}

impl<T: Component> ComponentStorage<T> for PackedStorage<T> {
    type Iter<'a> = ComponentIter<'a, T>;
    type IterMut<'a> = ComponentIterMut<'a, T>;

    unsafe fn extend_memcopy(&mut self, archetype: ArchetypeIndex, ptr: *const T, count: usize) {
        unsafe { self.extend_memcopy_raw(archetype, ptr.cast::<u8>(), count) };
    }

    fn get(&self, ArchetypeIndex(archetype): ArchetypeIndex) -> Option<ComponentSlice<'_, T>> {
        let slice_index = *self.index.get(archetype as usize)?;
        let (ptr, len) = self.slices.get(slice_index)?;
        let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), *len) };
        let version = unsafe { &*self.versions.get_unchecked(slice_index).get() };
        Some(ComponentSlice::new(slice, version))
    }

    unsafe fn get_mut(
        &self,
        ArchetypeIndex(archetype): ArchetypeIndex,
    ) -> Option<ComponentSliceMut<'_, T>> {
        let slice_index = *self.index.get(archetype as usize)?;
        let (ptr, len) = self.slices.get(slice_index)?;
        let slice = unsafe { std::slice::from_raw_parts_mut(ptr.as_ptr(), *len) };
        let version = unsafe { &mut *self.versions.get_unchecked(slice_index).get() };
        Some(ComponentSliceMut::new(slice, version))
    }

    fn iter(&self, start_inclusive: usize, end_exclusive: usize) -> Self::Iter<'_> {
        ComponentIter {
            slices: self.slices[start_inclusive..end_exclusive]
                .iter()
                .zip(self.versions[start_inclusive..end_exclusive].iter()),
        }
    }

    unsafe fn iter_mut(&self, start_inclusive: usize, end_exclusive: usize) -> Self::IterMut<'_> {
        ComponentIterMut {
            slices: self.slices[start_inclusive..end_exclusive]
                .iter()
                .zip(self.versions[start_inclusive..end_exclusive].iter()),
        }
    }

    fn len(&self) -> usize {
        self.allocations.len()
    }
}

#[doc(hidden)]
pub struct ComponentIter<'a, T> {
    slices: Zip<Iter<'a, (NonNull<T>, usize)>, Iter<'a, UnsafeCell<u64>>>,
}

impl<'a, T: Component> Iterator for ComponentIter<'a, T> {
    type Item = ComponentSlice<'a, T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.slices.next().map(|((ptr, len), version)| {
            let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), *len) };
            let version = unsafe { &*version.get() };
            ComponentSlice::new(slice, version)
        })
    }
}

#[doc(hidden)]
pub struct ComponentIterMut<'a, T> {
    slices: Zip<Iter<'a, (NonNull<T>, usize)>, Iter<'a, UnsafeCell<u64>>>,
}

impl<'a, T: Component> Iterator for ComponentIterMut<'a, T> {
    type Item = ComponentSliceMut<'a, T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.slices.next().map(|((ptr, len), version)| {
            // safety: we know each slice is disjoint
            let slice = unsafe { std::slice::from_raw_parts_mut(ptr.as_ptr(), *len) };
            let version = unsafe { &mut *version.get() };
            ComponentSliceMut::new(slice, version)
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // The element count at which `size_of::<T>() * count` overflows `usize`. The old unchecked
    // multiplication wrapped here in release builds, producing an undersized allocation.
    fn overflowing_count<T>() -> usize {
        usize::MAX / size_of::<T>() + 1
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn raw_alloc_new_rejects_overflowing_capacity() {
        drop(RawAlloc::<u64>::new(overflowing_count::<u64>()));
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn raw_alloc_grow_rejects_overflowing_capacity() {
        let mut alloc = RawAlloc::<u64>::new(1);
        alloc.grow(overflowing_count::<u64>());
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn raw_alloc_rejects_capacity_beyond_isize_max() {
        // Does not overflow `usize`, but `Layout` forbids sizes above `isize::MAX`.
        drop(RawAlloc::<u64>::new(
            isize::MAX as usize / size_of::<u64>() + 1,
        ));
    }

    #[test]
    fn raw_alloc_grow_preserves_contents_and_alignment() {
        // Both the fresh-allocation (cap == 0) and the realloc paths, with an over-aligned type.
        let mut alloc = RawAlloc::<u128>::new(0);
        alloc.grow(2);
        assert_eq!(alloc.cap, 2);
        unsafe {
            alloc.ptr.as_ptr().write(11);
            alloc.ptr.as_ptr().add(1).write(22);
        }

        alloc.grow(100);
        assert_eq!(alloc.cap, 100);
        assert!(alloc.ptr.as_ptr().is_aligned());
        unsafe {
            assert_eq!(alloc.ptr.as_ptr().read(), 11);
            assert_eq!(alloc.ptr.as_ptr().add(1).read(), 22);
            alloc.ptr.as_ptr().add(99).write(33);
            assert_eq!(alloc.ptr.as_ptr().add(99).read(), 33);
        }
    }

    #[test]
    fn raw_alloc_zero_sized_never_allocates() {
        let alloc = RawAlloc::<()>::new(5);
        assert_eq!(alloc.cap, !0);
        drop(alloc);
    }

    #[test]
    fn create_storage() {
        drop(PackedStorage::<usize>::default());
    }

    #[test]
    fn create_zst_storage() {
        drop(PackedStorage::<()>::default());
    }

    #[test]
    fn insert_archetype() {
        let mut storage = PackedStorage::<usize>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));
    }

    #[test]
    fn insert_zst_archetype() {
        let mut storage = PackedStorage::<()>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));
    }

    #[test]
    fn insert_components() {
        let mut storage = PackedStorage::<usize>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));

        unsafe {
            let components = vec![1, 2, 3];
            let ptr = components.as_ptr();
            storage.extend_memcopy(ArchetypeIndex(0), ptr, 3);
            std::mem::forget(components);

            let slice = storage.get_mut(ArchetypeIndex(0)).unwrap();
            assert_eq!(slice.into_slice(), &[1usize, 2usize, 3usize]);
        }
    }

    #[test]
    fn insert_zst_components() {
        let mut storage = PackedStorage::<()>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));

        unsafe {
            let components = vec![(), (), ()];
            let ptr = components.as_ptr();
            storage.extend_memcopy(ArchetypeIndex(0), ptr, 3);
            std::mem::forget(components);

            let slice = storage.get_mut(ArchetypeIndex(0)).unwrap();
            assert_eq!(slice.into_slice(), &[(), (), ()]);
        }
    }

    #[test]
    fn swap_remove_first() {
        let mut storage = PackedStorage::<usize>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));

        unsafe {
            let components = vec![1, 2, 3];
            let ptr = components.as_ptr();
            storage.extend_memcopy(ArchetypeIndex(0), ptr, 3);
            std::mem::forget(components);
        }

        storage.swap_remove(ArchetypeIndex(0), ComponentIndex(0));

        unsafe {
            let slice = storage.get_mut(ArchetypeIndex(0)).unwrap();
            assert_eq!(slice.into_slice(), &[3usize, 2usize]);
        }
    }

    #[test]
    fn swap_remove_last() {
        let mut storage = PackedStorage::<usize>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));

        unsafe {
            let components = vec![1, 2, 3];
            let ptr = components.as_ptr();
            storage.extend_memcopy(ArchetypeIndex(0), ptr, 3);
            std::mem::forget(components);
        }

        storage.swap_remove(ArchetypeIndex(0), ComponentIndex(2));

        unsafe {
            let slice = storage.get_mut(ArchetypeIndex(0)).unwrap();
            assert_eq!(slice.into_slice(), &[1usize, 2usize]);
        }
    }

    /// Fragmentation divides by `entity_len`,
    /// so an empty storage yields NaN (no archetypes: 0/0)
    /// or infinity (empty loose archetypes: 1/0).
    #[test]
    fn fragmentation_of_empty_storage_is_finite() {
        let no_archetypes = PackedStorage::<usize>::default();
        assert!(no_archetypes.fragmentation().is_finite());

        let mut storage = PackedStorage::<usize>::default();
        storage.insert_archetype(ArchetypeIndex(0), Some(0));
        assert!(storage.fragmentation().is_finite());
    }

    // ---- Tests checking overflows and leaks ----

    fn loose_capacity<T>(vec: &ComponentVec<T>) -> usize {
        match vec {
            ComponentVec::Loose { raw, .. } => raw.cap,
            ComponentVec::Packed { cap, .. } => *cap,
        }
    }

    /// Overflow test: growing a `Packed` vec allocates `len` elements instead of `new_capacity`, so the
    /// resulting `Loose` vec has no room for the elements the caller asked for. The following
    /// `extend_memcopy` then writes past the end of the buffer (heap overflow).
    ///
    /// This test checks the capacity invariant only, so it does not itself trigger the overflow.
    #[test]
    fn packed_grow_provides_requested_capacity() {
        let mut vec = ComponentVec::<u64>::new();
        let data = [1u64, 2, 3];
        unsafe {
            vec.extend_memcopy(0, data.as_ptr(), data.len());
            vec.pack(Rc::new(RawAlloc::new(3)), 0);
        }
        assert!(matches!(vec, ComponentVec::Packed { len: 3, cap: 3, .. }));

        // Ask for room for one more element (this is what `extend_memcopy` does first).
        vec.ensure_capacity(0, 1);

        let len = vec.len();
        let cap = loose_capacity(&vec);
        assert!(
            cap - len >= 1,
            "no room for the requested element after growing: len = {len}, cap = {cap}"
        );
        assert_eq!(&*vec, &[1u64, 2, 3]);
    }

    /// Overflow test: same cause, directly through `grow`.
    #[test]
    fn packed_grow_allocates_new_capacity() {
        let mut vec = ComponentVec::<u64>::new();
        let data = [1u64, 2, 3];
        unsafe {
            vec.extend_memcopy(0, data.as_ptr(), data.len());
            vec.pack(Rc::new(RawAlloc::new(3)), 0);
        }

        vec.grow(0, 10);

        assert!(
            loose_capacity(&vec) >= 10,
            "grow(10) produced capacity {}",
            loose_capacity(&vec)
        );
    }

    /// Leak test: the slow path of `transfer_archetype` `mem::forget`s the old source vec.
    /// That is right for the elements (they were memcopied) but it also leaks the buffer,
    /// and for a packed vec it leaks an `Rc` reference so the shared allocation is never freed.
    #[test]
    fn transfer_archetype_slow_path_does_not_leak_packed_allocation() {
        fn insert_usizes(
            storage: &mut PackedStorage<usize>,
            archetype: ArchetypeIndex,
            values: &[usize],
        ) {
            unsafe { storage.extend_memcopy(archetype, values.as_ptr(), values.len()) };
        }

        let mut src = PackedStorage::<usize>::default();
        src.insert_archetype(ArchetypeIndex(0), Some(0));
        insert_usizes(&mut src, ArchetypeIndex(0), &[1, 2, 3]);
        src.pack(0);

        // Hold our own reference to the shared allocation so we can observe its refcount.
        let shared = match &src.allocations[0] {
            ComponentVec::Packed { raw, .. } => Rc::clone(raw),
            ComponentVec::Loose { .. } => panic!("expected the archetype to be packed"),
        };
        assert_eq!(Rc::strong_count(&shared), 2); // the vec + us

        // Destination is non-empty, which forces the memcopy (slow) path.
        let mut dst = PackedStorage::<usize>::default();
        dst.insert_archetype(ArchetypeIndex(0), Some(0));
        insert_usizes(&mut dst, ArchetypeIndex(0), &[10]);

        src.transfer_archetype(ArchetypeIndex(0), ArchetypeIndex(0), &mut dst);

        // The data arrived correctly...
        unsafe {
            let slice = dst.get_mut(ArchetypeIndex(0)).unwrap();
            assert_eq!(slice.into_slice(), &[10usize, 1, 2, 3]);
        }
        assert_eq!(src.get(ArchetypeIndex(0)).unwrap().into_slice().len(), 0);

        // ...and the source no longer references the shared buffer, so only we do.
        assert_eq!(
            Rc::strong_count(&shared),
            1,
            "the old source vec was forgotten, leaking its Rc reference"
        );
    }
}
