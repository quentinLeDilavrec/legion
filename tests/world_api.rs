use std::collections::HashSet;

use legion::*;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pos(f32, f32, f32);
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rot(f32, f32, f32);
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scale(f32, f32, f32);

#[test]
fn insert() {
    let mut world = World::default();

    let components = vec![(4f32, 5u64, 6u16), (4f32, 5u64, 6u16)];
    let entities = world.extend(components);

    assert_eq!(2, entities.len());
}

#[test]
fn get_component() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let mut entities: Vec<Entity> = Vec::new();
    for e in world.extend(components.clone()) {
        entities.push(*e);
    }

    for (i, e) in entities.iter().enumerate() {
        match world.entry_ref(*e).unwrap().get_component::<Pos>() {
            Ok(x) => assert_eq!(components.get(i).map(|(x, _)| x), Some(x)),
            Err(_) => assert_eq!(components.get(i).map(|(x, _)| x), None),
        }
        match world.entry_ref(*e).unwrap().get_component::<Rot>() {
            Ok(x) => assert_eq!(components.get(i).map(|(_, x)| x), Some(x)),
            Err(_) => assert_eq!(components.get(i).map(|(_, x)| x), None),
        }
    }
}

#[test]
fn get_component_wrong_type() {
    let mut world = World::default();

    let entity = *world.extend(vec![(0f64,)]).first().unwrap();

    assert!(world
        .entry_ref(entity)
        .unwrap()
        .get_component::<i32>()
        .is_err());
}

#[test]
fn remove() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let mut entities: Vec<Entity> = Vec::new();
    for e in world.extend(components) {
        entities.push(*e);
    }

    for e in entities.iter() {
        assert!(world.contains(*e));
    }

    for e in entities.iter() {
        world.remove(*e);
        assert!(!world.contains(*e));
    }
}

#[test]
fn delete_all() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let mut entities: Vec<Entity> = Vec::new();
    for e in world.extend(components) {
        entities.push(*e);
    }

    // Check that the entity allocator knows about the entities
    for e in entities.iter() {
        assert!(world.contains(*e));
    }

    // Check that the entities are in storage
    let mut query = <(Read<Pos>, Read<Rot>)>::query();
    assert_eq!(2, query.iter(&world).count());

    world.clear();

    // Check that the entity allocator no longer knows about the entities
    for e in entities.iter() {
        assert!(!world.contains(*e));
    }

    // Check that the entities are removed from storage
    let mut query = <(Read<Pos>, Read<Rot>)>::query();
    assert_eq!(0, query.iter(&world).count());
}

#[test]
fn delete_last() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let mut entities: Vec<Entity> = Vec::new();
    for e in world.extend(components.clone()) {
        entities.push(*e);
    }

    let last = *entities.last().unwrap();
    world.remove(last);
    assert!(!world.contains(last));

    for (i, e) in entities.iter().take(entities.len() - 1).enumerate() {
        assert!(world.contains(*e));
        match world.entry_ref(*e).unwrap().get_component::<Pos>() {
            Ok(x) => assert_eq!(components.get(i).map(|(x, _)| x), Some(x)),
            Err(_) => assert_eq!(components.get(i).map(|(x, _)| x), None),
        }
        match world.entry_ref(*e).unwrap().get_component::<Rot>() {
            Ok(x) => assert_eq!(components.get(i).map(|(_, x)| x), Some(x)),
            Err(_) => assert_eq!(components.get(i).map(|(_, x)| x), None),
        }
    }
}

#[test]
fn delete_first() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let mut entities: Vec<Entity> = Vec::new();
    for e in world.extend(components.clone()) {
        entities.push(*e);
    }

    let first = *entities.first().unwrap();

    world.remove(first);
    assert!(!world.contains(first));

    for (i, e) in entities.iter().skip(1).enumerate() {
        assert!(world.contains(*e));
        match world.entry_ref(*e).unwrap().get_component::<Pos>() {
            Ok(x) => assert_eq!(components.get(i + 1).map(|(x, _)| x), Some(x)),
            Err(_) => assert_eq!(components.get(i + 1).map(|(x, _)| x), None),
        }
        match world.entry_ref(*e).unwrap().get_component::<Rot>() {
            Ok(x) => assert_eq!(components.get(i + 1).map(|(_, x)| x), Some(x)),
            Err(_) => assert_eq!(components.get(i + 1).map(|(_, x)| x), None),
        }
    }
}

#[test]
fn merge() {
    let mut world_1 = World::default();
    let mut world_2 = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let mut world_1_entities: Vec<Entity> = Vec::new();
    for e in world_1.extend(components.clone()) {
        world_1_entities.push(*e);
    }

    let mut world_2_entities: Vec<Entity> = Vec::new();
    for e in world_2.extend(components.clone()) {
        world_2_entities.push(*e);
    }

    world_1.move_from(&mut world_2, &any());

    for (i, e) in world_2_entities.iter().enumerate() {
        assert!(world_1.contains(*e));

        let (pos, rot) = components.get(i).unwrap();
        assert_eq!(
            pos,
            world_1.entry(*e).unwrap().get_component::<Pos>().unwrap()
        );
        assert_eq!(
            rot,
            world_1.entry(*e).unwrap().get_component::<Rot>().unwrap()
        );
    }
}

#[test]
fn mutate_add_component() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let entities = world.extend(components).to_vec();

    let mut query_without_scale = <(Read<Pos>, Read<Rot>)>::query();
    let mut query_with_scale = <(Read<Pos>, Read<Rot>, Read<Scale>)>::query();

    assert_eq!(3, query_without_scale.iter(&world).count());
    assert_eq!(0, query_with_scale.iter(&world).count());

    world
        .entry(entities[1])
        .unwrap()
        .add_component(Scale(0.5, 0.5, 0.5));

    assert_eq!(3, query_without_scale.iter(&world).count());
    assert_eq!(1, query_with_scale.iter(&world).count());
}

#[test]
fn mutate_remove_component() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    let entities = world.extend(components).to_vec();

    let mut query_without_rot = Read::<Pos>::query().filter(!component::<Rot>());
    let mut query_with_rot = <(Read<Pos>, Read<Rot>)>::query();

    assert_eq!(0, query_without_rot.iter(&world).count());
    assert_eq!(3, query_with_rot.iter(&world).count());

    world.entry(entities[1]).unwrap().remove_component::<Rot>();

    assert_eq!(1, query_without_rot.iter(&world).count());
    assert_eq!(2, query_with_rot.iter(&world).count());
}

#[test]
#[cfg(feature = "events")]
fn delete_entities_on_drop() {
    let (tx, rx) = std::sync::mpsc::channel::<legion::world::Event>();
    delete_entities_on_drop_impl(tx, rx);
}

#[test]
#[cfg(feature = "events")]
fn sync_sender_survives_full_channel() {
    let mut world = World::default();
    let (tx, rx) = std::sync::mpsc::sync_channel::<legion::world::Event>(1);
    world.subscribe(tx, any());

    // More events than the channel can hold: extras are dropped without panicking.
    world.extend(vec![(Pos(1., 2., 3.),), (Pos(4., 5., 6.),)]);
    std::mem::drop(world);

    assert!(rx.try_iter().count() >= 1);
}

#[test]
#[cfg(feature = "events")]
fn closure_and_collector_subscribers() {
    use std::sync::{Arc, Mutex};

    let mut world = World::default();

    let collected = Arc::new(Mutex::new(Vec::<legion::world::Event>::new()));
    world.subscribe(Arc::clone(&collected), any());

    let (tx, rx) = std::sync::mpsc::channel::<legion::world::Event>();
    world.subscribe(move |event| tx.send(event).is_ok(), any());

    world.push((Pos(1., 2., 3.),));

    let from_collector = collected.lock().unwrap().len();
    assert!(from_collector >= 1);
    assert_eq!(from_collector, rx.try_iter().count());
}

#[test]
#[cfg(feature = "events")]
fn closure_subscriber_dropped_when_dead() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let mut world = World::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = Arc::clone(&calls);
    world.subscribe(
        move |_| {
            calls2.fetch_add(1, Ordering::SeqCst);
            false
        },
        any(),
    );

    world.push((Pos(1., 2., 3.),));
    let after_first = calls.load(Ordering::SeqCst);
    assert!(after_first >= 1);

    // Returning `false` unsubscribes from that archetype, so further events there are not sent.
    world.push((Pos(1., 2., 3.),));
    world.push((Pos(1., 2., 3.),));
    assert_eq!(calls.load(Ordering::SeqCst), after_first);
}

#[cfg(feature = "events")]
fn delete_entities_on_drop_impl<S, R>(tx: S, rx: R)
where
    S: legion::world::EventSender + 'static,
    R: IntoIterator<Item = legion::world::Event>,
{
    let mut world = World::default();

    let components = vec![(Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3))];

    // Insert the data and store resulting entities in a HashSet
    let mut entities = HashSet::new();
    for entity in world.extend(components) {
        entities.insert(*entity);
    }

    world.subscribe(tx, any());

    std::mem::drop(world);

    for e in rx {
        println!("{:?}", e);
        if let legion::world::Event::EntityRemoved(entity, _arch_id) = e {
            assert!(entities.remove(&entity));
        }
    }

    // Verify that no extra entities are included
    assert!(entities.is_empty());
}

// This test repeatedly creates a world with new entities and drops it, reproducing
// https://github.com/TomGillen/legion/issues/92
#[test]
fn lots_of_deletes() {
    for _ in 0..10000 {
        let components = vec![
            (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
            (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        ];

        let mut world = World::default();
        world.extend(components).to_vec();
    }
}

#[test]
fn iter_entities() {
    let mut world = World::default();

    let components = vec![
        (Pos(1., 2., 3.), Rot(0.1, 0.2, 0.3)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
        (Pos(4., 5., 6.), Rot(0.4, 0.5, 0.6)),
    ];

    // Insert the data and store resulting entities in a HashSet
    let mut entities = HashSet::new();
    for entity in world.extend(components) {
        entities.insert(*entity);
    }

    // Verify that all entities in iter_entities() are included
    let mut all = Entity::query();
    for entity in all.iter(&world) {
        assert!(entities.remove(entity));
    }

    // Verify that no extra entities are included
    assert!(entities.is_empty());
}
