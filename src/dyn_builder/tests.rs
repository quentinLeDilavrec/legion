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
