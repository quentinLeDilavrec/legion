//! World-level test for potential memory bugs in `PackedStorage`
//! (`src/internals/storage/packed.rs`).
//!
//! Both considered bugs are memory-level problems (a heap overflow and a leak),
//! which ordinary assertions cannot see.
//! This test binary therefore installs a small global allocator for itself that:
//! * places a canary region after every allocation and checks it on `dealloc`, which reveals
//!   writes past the end of a block, and
//! * counts live allocations, which reveals leaks.
//!
//! Both counters are thread-local, so tests running in parallel do not affect each other.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use legion::{storage::PackOptions, *};

const GUARD: usize = 64;
const CANARY: u8 = 0xAB;

thread_local! {
    static LIVE_ALLOCATIONS: Cell<isize> = const { Cell::new(0) };
    static CANARY_VIOLATIONS: Cell<usize> = const { Cell::new(0) };
}

fn live_allocations() -> isize {
    LIVE_ALLOCATIONS.with(Cell::get)
}

fn canary_violations() -> usize {
    CANARY_VIOLATIONS.with(Cell::get)
}

struct GuardedAlloc;

fn guarded(layout: Layout) -> Layout {
    Layout::from_size_align(layout.size() + GUARD, layout.align()).unwrap()
}

unsafe impl GlobalAlloc for GuardedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            let ptr = System.alloc(guarded(layout));
            if !ptr.is_null() {
                std::ptr::write_bytes(ptr.add(layout.size()), CANARY, GUARD);
                LIVE_ALLOCATIONS.with(|c| c.set(c.get() + 1));
            }
            ptr
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            let canary = std::slice::from_raw_parts(ptr.add(layout.size()), GUARD);
            if canary.iter().any(|b| *b != CANARY) {
                CANARY_VIOLATIONS.with(|c| c.set(c.get() + 1));
            }
            LIVE_ALLOCATIONS.with(|c| c.set(c.get() - 1));
            System.dealloc(ptr, guarded(layout));
        }
    }
}

#[global_allocator]
static ALLOCATOR: GuardedAlloc = GuardedAlloc;

/// The payload component.
///
/// Tests assert on its value after packing, moving and merging,
/// to check that data survives even when the buffer underneath it is misused.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Val(u64);

/// A second component that exists to create a second archetype.
///
/// Adding it to an entity that only has `Val` moves the entity from `(Val,)` to `(Val, Tag)`.
///
/// Despite the name, this is not a zero-sized marker.
/// It wraps a `u64` on purpose, because `PackedStorage::pack` skips zero-sized types,
/// so a ZST would never be packed and could not reproduce the bug.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tag(u64);

fn val_of(world: &mut World, entity: Entity) -> u64 {
    world
        .entry(entity)
        .unwrap()
        .get_component::<Val>()
        .unwrap()
        .0
}

/// Appending to a packed archetype overflows its buffer.
///
/// `PackedStorage` packs archetypes into windows with `cap == len`. The next append has to move
/// the archetype out of the shared buffer (`ComponentVec::grow`), but it only allocates `len`
/// elements for the new private buffer, then writes the new element past the end of it.
///
/// This test goes through `Entry::add_component`, which calls `extend_memcopy` directly.
/// Other insertion paths may be shielded when they call `ensure_capacity` with an accurate size first
/// (I have not checked this), but any path that appends without it hits the bug.
#[test]
fn add_component_to_packed_archetype_overflows_buffer() {
    {
        let mut world = World::default();
        let moving = world.push((Val(1),));
        let resident = world.push((Val(2), Tag(20)));

        // Pack both `Val` windows (and the `Tag` window) into shared buffers, each with cap == len.
        world.pack(PackOptions::force());

        // Moves `Val` into the packed `(Val, Tag)` archetype,
        // and appends the new `Tag` to the packed `Tag` storage.
        // Both appends overflow.
        world.entry(moving).unwrap().add_component(Tag(10));

        // The overflow does not corrupt the values we can see.
        assert_eq!(val_of(&mut world, moving), 1);
        assert_eq!(val_of(&mut world, resident), 2);

        // ...it writes into memory beyond the buffer.
        // The world is dropped here,
        // which frees the buffers and lets the allocator check their canaries.
    }

    assert_eq!(
        canary_violations(),
        0,
        "a component buffer was written past its end"
    );
}

/// Runs a world merge in which the destination archetype is non-empty, so
/// `PackedStorage::transfer_archetype` takes its memcopy ("slow") path.
fn merge_into_non_empty_archetype(pack_source: bool) {
    let mut dst = World::default();
    dst.push((Val(100),));

    let mut src = World::default();
    src.extend(vec![(Val(0),), (Val(1),), (Val(2),), (Val(3),)]);
    if pack_source {
        src.pack(PackOptions::force());
    }

    dst.move_from(&mut src, &any());

    let mut values: Vec<u64> = <&Val>::query().iter(&dst).map(|v| v.0).collect();
    values.sort_unstable();
    assert_eq!(values, [0, 1, 2, 3, 100]);
    assert_eq!(<&Val>::query().iter(&src).count(), 0);
}

fn assert_merge_does_not_leak(pack_source: bool) {
    // Warm up so that one-time lazy allocations are not mistaken for leaks.
    merge_into_non_empty_archetype(pack_source);

    let before = live_allocations();
    merge_into_non_empty_archetype(pack_source);
    let leaked = live_allocations() - before;

    assert_eq!(
        leaked, 0,
        "{leaked} allocation(s) were still live after both worlds were dropped"
    );
}

/// Leak test (loose source): the slow path of `transfer_archetype` `mem::forget`s the old source vec.
/// The elements were memcopied so skipping their destructors is right,
/// but forgetting the whole vec also leaks its buffer.
#[test]
fn merging_worlds_does_not_leak_loose_source_buffer() {
    assert_merge_does_not_leak(false);
}

/// Leak test (packed source): same as above, but the forgotten vec was `Packed`,
/// so it leaks a reference to the shared allocation, which therefore is never freed.
#[test]
fn merging_worlds_does_not_leak_packed_source_buffer() {
    assert_merge_does_not_leak(true);
}
