use std::any::TypeId;

use super::builder::EntityBuilder;

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

// Components must be moved into the world, never deep-cloned or dropped twice.
// We check this by comparing heap addresses captured *before* the move
// (never dereferencing a pointer to a moved-from value)
// and by counting `Arc` owners.
#[test]
fn test_cloning() {
    use std::sync::Arc;

    let mut world = crate::World::new(Default::default());
    let mut components = EntityBuilder::default();

    let comp0: (Box<[u32]>,) = (vec![0, 0, 0, 0, 0, 1, 4100177920].into_boxed_slice(),);
    let comp0_saved = comp0.clone();
    let comp0_addr = comp0.0.as_ptr() as usize;
    components.add(comp0);

    let comp1: i32 = 0;
    components.add(comp1);
    let comp2: bool = true;
    components.add(comp2);

    let comp3: Vec<u64> = vec![0, 1, 2, 3];
    let comp3_saved = comp3.clone();
    let comp3_addr = comp3.as_ptr() as usize;
    components.add(comp3);

    let comp4: String = "ewgwgwsegwesf".into();
    let comp4_addr = comp4.as_ptr() as usize;
    components.add(comp4.clone());

    let shared = Arc::new(7u32);
    components.add(shared.clone());
    assert_eq!(Arc::strong_count(&shared), 2);

    let comp5: u64 = 0;
    components.add(comp5);

    let entity = world.extend(components)[0];
    assert_eq!(Arc::strong_count(&shared), 2, "extend must move, not clone");

    let entry = world.entry(entity).unwrap();
    let got0 = entry.get_component::<(Box<[u32]>,)>().unwrap();
    assert_eq!(got0, &comp0_saved);
    assert_eq!(
        got0.0.as_ptr() as usize,
        comp0_addr,
        "slice should have been moved"
    );
    assert_eq!(Ok(&comp1), entry.get_component::<i32>());
    assert_eq!(Ok(&comp2), entry.get_component::<bool>());
    let got3 = entry.get_component::<Vec<u64>>().unwrap();
    assert_eq!(got3, &comp3_saved);
    assert_eq!(
        got3.as_ptr() as usize,
        comp3_addr,
        "vec should have been moved"
    );
    // `comp4` was cloned on purpose, so the world's copy must own a different buffer.
    let got4 = entry.get_component::<String>().unwrap();
    assert_eq!(got4, &comp4);
    assert_ne!(got4.as_ptr() as usize, comp4_addr);
    assert!(Arc::ptr_eq(
        entry.get_component::<Arc<u32>>().unwrap(),
        &shared
    ));

    drop(world);
    assert_eq!(
        Arc::strong_count(&shared),
        1,
        "world must drop it exactly once"
    );
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
fn test_mut_or_default_drop_type() {
    // Heap-owning component: a stray extra `T::default()` copy would be double-freed.
    let mut components = EntityBuilder::default();
    components.mut_or_default::<Vec<u8>>().push(1);
    components.mut_or_default::<Vec<u8>>().push(2);
    assert_eq!(components.get_mut::<Vec<u8>>(), Some(&mut vec![1, 2]));

    let mut world = crate::World::new(Default::default());
    let entity = world.extend(components)[0];
    assert_eq!(
        Ok(&vec![1u8, 2]),
        world.entry(entity).unwrap().get_component::<Vec<u8>>()
    );
}

#[test]
fn test_mut_or_default_rc() {
    use std::rc::Rc;

    #[derive(Default)]
    struct Tracked(Rc<()>);
    // Needed because `Component` requires `Send + Sync`; only used on this thread.
    unsafe impl Send for Tracked {}
    unsafe impl Sync for Tracked {}

    let rc = Rc::new(());
    {
        let mut components = EntityBuilder::default();
        let tracked = components.mut_or_default::<Tracked>();
        tracked.0 = rc.clone();
        assert_eq!(Rc::strong_count(&rc), 2);
        let tracked = components.mut_or_default::<Tracked>();
        assert!(Rc::ptr_eq(&tracked.0, &rc));
        assert_eq!(Rc::strong_count(&rc), 2);

        let mut world = crate::World::new(Default::default());
        let entity = world.extend(components)[0];
        assert_eq!(Rc::strong_count(&rc), 2);
        world.remove(entity);
    }
    assert_eq!(Rc::strong_count(&rc), 1);
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
