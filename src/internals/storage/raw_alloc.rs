//! A raw, untyped-length heap allocation for an array of `T`.
//!
//! [`RawAlloc`] knows how to allocate, grow and free a block of memory for `capacity` elements of
//! `T`, and nothing else: it has no notion of a length and never reads, writes or drops elements.
//! That makes it a building block for component storages (or any other array-like container) that
//! want to manage their own initialised range.

use std::{alloc::Layout, mem::size_of, ptr::NonNull};

/// A memory allocation for an array of `T`.
///
/// The memory is uninitialised, and elements are never dropped by this type:
/// freeing the allocation (on drop) is the only thing it does.
/// The owner is responsible for tracking which elements are initialised and for dropping them.
///
/// Zero-sized `T`s never allocate; their capacity is reported as `usize::MAX`.
#[derive(Debug)]
pub struct RawAlloc<T> {
    ptr: NonNull<T>,
    cap: usize,
}

impl<T> RawAlloc<T> {
    /// Allocates room for at least `min_capacity` elements.
    ///
    /// A capacity of zero does not allocate.
    ///
    /// # Panics
    /// Panics if the total size in bytes overflows `usize` or exceeds `isize::MAX`.
    pub fn new(min_capacity: usize) -> Self {
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

    /// Returns a pointer to the start of the allocation.
    ///
    /// The pointer is dangling (but aligned and non-null) when `capacity() == 0` or `T` is
    /// zero-sized, and must not be dereferenced in that case. The pointer is invalidated by
    /// [`grow`](Self::grow).
    pub fn ptr(&self) -> NonNull<T> {
        self.ptr
    }

    /// Returns the number of elements the allocation has room for.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Grows the allocation to hold `new_capacity` elements, preserving the existing contents
    /// (the first `capacity()` elements) of the allocation.
    ///
    /// # Panics
    /// Panics if the total size in bytes overflows `usize` or exceeds `isize::MAX`.
    /// In debug builds, also panics if `T` is zero-sized or `new_capacity <= capacity()`.
    pub fn grow(&mut self, new_capacity: usize) {
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
        Layout::array::<T>(capacity).expect("raw allocation capacity overflow")
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
    fn new_rejects_overflowing_capacity() {
        drop(RawAlloc::<u64>::new(overflowing_count::<u64>()));
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn grow_rejects_overflowing_capacity() {
        let mut alloc = RawAlloc::<u64>::new(1);
        alloc.grow(overflowing_count::<u64>());
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn rejects_capacity_beyond_isize_max() {
        // Does not overflow `usize`, but `Layout` forbids sizes above `isize::MAX`.
        drop(RawAlloc::<u64>::new(
            isize::MAX as usize / size_of::<u64>() + 1,
        ));
    }

    #[test]
    fn grow_preserves_contents_and_alignment() {
        // Both the fresh-allocation (cap == 0) and the realloc paths, with an over-aligned type.
        let mut alloc = RawAlloc::<u128>::new(0);
        alloc.grow(2);
        assert_eq!(alloc.capacity(), 2);
        unsafe {
            alloc.ptr().as_ptr().write(11);
            alloc.ptr().as_ptr().add(1).write(22);
        }

        alloc.grow(100);
        assert_eq!(alloc.capacity(), 100);
        assert!(alloc.ptr().as_ptr().is_aligned());
        unsafe {
            assert_eq!(alloc.ptr().as_ptr().read(), 11);
            assert_eq!(alloc.ptr().as_ptr().add(1).read(), 22);
            alloc.ptr().as_ptr().add(99).write(33);
            assert_eq!(alloc.ptr().as_ptr().add(99).read(), 33);
        }
    }

    #[test]
    fn zero_capacity_does_not_allocate() {
        let alloc = RawAlloc::<u64>::new(0);
        assert_eq!(alloc.capacity(), 0);
        drop(alloc);
    }

    #[test]
    fn zero_sized_never_allocates() {
        let alloc = RawAlloc::<()>::new(5);
        assert_eq!(alloc.capacity(), !0);
        drop(alloc);
    }
}
