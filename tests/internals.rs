//! Smoke test for the `internals` feature: uses legion's building blocks the way a crate
//! building its own ECS would, going only through `legion::internals`.

use std::sync::Mutex;

use legion::internals::{
    entity::{Allocate, EntityLocation, LocationMap},
    event::{Event, EventSender},
    storage::{
        archetype::ArchetypeIndex, component::ComponentTypeId, component_set::ComponentSet,
        index::SearchIndex, next_component_version, packed::PackedStorage, raw_alloc::RawAlloc,
        ComponentIndex, ComponentMeta, ComponentSlice, ComponentStorage, UnknownComponentStorage,
    },
};

#[test]
fn entity_ids_and_locations() {
    let mut ids = Allocate::new();
    let mut locations = LocationMap::default();

    let a = ids.next().unwrap();
    let b = ids.next().unwrap();
    assert_ne!(a, b);

    locations.insert(&[a, b], ArchetypeIndex(3), ComponentIndex::new(10));

    let location = locations.get(b).unwrap();
    assert_eq!(location.archetype(), ArchetypeIndex(3));
    assert_eq!(location.component().get(), 11);

    locations.set(
        a,
        EntityLocation::new(ArchetypeIndex(4), ComponentIndex::new(0)),
    );
    assert_eq!(locations.get(a).unwrap().archetype(), ArchetypeIndex(4));
    assert_eq!(locations.len(), 2);
}

#[test]
fn use_a_component_storage_directly() {
    let mut storage = PackedStorage::<u32>::default();
    let archetype = ArchetypeIndex(0);
    storage.insert_archetype(archetype, None);

    let values = [1u32, 2, 3];
    unsafe { storage.extend_memcopy(archetype, values.as_ptr(), values.len()) };

    assert_eq!(storage.get(archetype).unwrap().into_slice(), &[1, 2, 3]);

    storage.swap_remove(archetype, ComponentIndex::new(0));
    assert_eq!(storage.get(archetype).unwrap().into_slice(), &[3, 2]);
}

#[test]
#[cfg(feature = "internals")]
fn custom_storages_can_build_slices_and_versions() {
    let data = [7u8, 8, 9];
    let version = next_component_version();
    assert!(next_component_version() > version);

    let slice = ComponentSlice::new(&data, &version);
    assert_eq!(slice.into_slice(), &data);
}

#[test]
fn raw_alloc_is_usable_on_its_own() {
    let mut alloc = RawAlloc::<u32>::new(2);
    assert_eq!(alloc.capacity(), 2);

    unsafe {
        alloc.ptr().as_ptr().write(1);
        alloc.ptr().as_ptr().add(1).write(2);
    }
    alloc.grow(8);

    assert_eq!(alloc.capacity(), 8);
    unsafe {
        assert_eq!(alloc.ptr().as_ptr().read(), 1);
        assert_eq!(alloc.ptr().as_ptr().add(1).read(), 2);
    }
}

#[test]
fn component_sets_and_search_index_need_no_world() {
    struct A;
    struct B;
    struct C;

    let ab: ComponentSet = [ComponentTypeId::of::<A>(), ComponentTypeId::of::<B>()]
        .into_iter()
        .collect();
    let b: ComponentSet = [ComponentTypeId::of::<B>()].into_iter().collect();
    let abc: ComponentSet = [
        ComponentTypeId::of::<A>(),
        ComponentTypeId::of::<B>(),
        ComponentTypeId::of::<C>(),
    ]
    .into_iter()
    .collect();

    let mut index = SearchIndex::default();
    index.push(&ab);
    index.push(&b);
    index.push(&abc);

    // a ComponentSet is an exact-match layout filter
    let found: Vec<_> = index.search(&b).collect();
    assert_eq!(found, [ArchetypeIndex(1)]);
    let found: Vec<_> = index.search(&abc).collect();
    assert_eq!(found, [ArchetypeIndex(2)]);
}

#[test]
fn component_meta_does_not_need_the_component_trait_machinery() {
    let meta = ComponentMeta::of::<[u16; 3]>();
    assert_eq!(meta.size(), 6);
    assert_eq!(meta.align(), 2);
}

#[test]
fn events_work_with_a_custom_id_type() {
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct MyId(u32);

    let sink = Mutex::new(Vec::<Event<MyId>>::new());
    assert!(sink.send(Event::EntityInserted(MyId(7), ArchetypeIndex(1))));

    let events = sink.into_inner().unwrap();
    assert!(matches!(
        events.as_slice(),
        [Event::EntityInserted(MyId(7), ArchetypeIndex(1))]
    ));
}

#[test]
fn only_packing_storages_expose_packing() {
    let mut storage = PackedStorage::<u32>::default();
    let packable = storage.as_packable().expect("PackedStorage can be packed");
    packable.increment_epoch();
    assert_eq!(packable.fragmentation(), 0.0);
}

#[test]
fn archetypes_can_be_built_with_a_custom_id_type() {
    use std::sync::Arc;

    use legion::{
        internals::{
            event::{Subscriber, Subscribers},
            storage::archetype::{Archetype, EntityLayout},
        },
        query::Any,
    };

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct MyId(u32);

    let sink = Arc::new(Mutex::new(Vec::<Event<MyId>>::new()));
    let mut subscribers = Subscribers::<MyId>::default();
    subscribers.push(Subscriber::new(Any, Arc::clone(&sink)));

    let archetype = Archetype::<MyId>::new(ArchetypeIndex(3), EntityLayout::new(), subscribers);

    assert_eq!(archetype.index(), ArchetypeIndex(3));
    assert!(archetype.entities().is_empty());
    assert!(archetype.layout().component_types().is_empty());

    // creating the archetype notified the subscriber
    let events = sink.lock().unwrap();
    assert!(matches!(
        events.as_slice(),
        [Event::ArchetypeCreated(ArchetypeIndex(3))]
    ));

    drop(events);

    // the remaining methods keep component positions aligned and notify subscribers
    let mut archetype = archetype;
    archetype.reserve(3);
    archetype.push(MyId(10));
    archetype.push(MyId(11));
    archetype.push(MyId(12));
    assert_eq!(archetype.entities(), &[MyId(10), MyId(11), MyId(12)]);

    // the last entity takes the removed one's place
    assert_eq!(archetype.swap_remove(0), MyId(10));
    assert_eq!(archetype.entities(), &[MyId(12), MyId(11)]);

    // a late subscriber is brought up to date with what is already there
    let late = Arc::new(Mutex::new(Vec::<Event<MyId>>::new()));
    archetype.subscribe(Subscriber::new(Any, Arc::clone(&late)));
    assert!(matches!(
        late.lock().unwrap().as_slice(),
        [
            Event::ArchetypeCreated(ArchetypeIndex(3)),
            Event::EntityInserted(MyId(12), _),
            Event::EntityInserted(MyId(11), _),
        ]
    ));

    assert_eq!(archetype.drain(), [MyId(12), MyId(11)]);
    assert!(archetype.entities().is_empty());

    let removed = sink
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, Event::EntityRemoved(..)))
        .count();
    assert_eq!(removed, 3); // one swap_remove, two from drain
}
