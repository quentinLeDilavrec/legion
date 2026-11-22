//! Smoke test for the `events` feature through `legion::internals`: uses events and subscribers
//! the way a crate building its own ECS would.

use std::sync::{Arc, Mutex};

use legion::{
    internals::{
        event::{Event, EventSender, Subscriber, Subscribers},
        storage::archetype::{Archetype, ArchetypeIndex, EntityLayout},
    },
    query::Any,
};

#[derive(Debug, Clone, Copy, PartialEq)]
struct MyId(u32);

#[test]
fn events_work_with_a_custom_id_type() {
    let sink = Mutex::new(Vec::<Event<MyId>>::new());
    assert!(sink.send(Event::EntityInserted(MyId(7), ArchetypeIndex(1))));

    let events = sink.into_inner().unwrap();
    assert!(matches!(
        events.as_slice(),
        [Event::EntityInserted(MyId(7), ArchetypeIndex(1))]
    ));
}

#[test]
fn archetypes_notify_subscribers_with_a_custom_id_type() {
    let sink = Arc::new(Mutex::new(Vec::<Event<MyId>>::new()));
    let mut subscribers = Subscribers::<MyId>::default();
    subscribers.push(Subscriber::new(Any, Arc::clone(&sink)));

    let archetype =
        Archetype::<MyId>::with_subscribers(ArchetypeIndex(3), EntityLayout::new(), subscribers);

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
