use std::{panic::AssertUnwindSafe, sync::Arc};

use legion::internals::storage::component::ComponentTypeId;

use crate::{
    id::AppendId,
    index::{Inserted, UniqueIndex},
    world::{AppendWorld, Bundle, KeyView, MutError},
    Color, ExternalId, Name, Pos, Vel,
};

fn pos(x: f32) -> Pos {
    Pos { x, y: 0.0 }
}

fn name(s: &str) -> Name {
    Name(s.to_string())
}

/// A world whose entities are keyed by their (immutable) name. Every entity a test inserts has a
/// distinct `Name`.
fn named_world() -> (AppendWorld, UniqueIndex<String>) {
    let mut world = AppendWorld::default();
    world.freeze::<Name>();
    (world, UniqueIndex::new())
}

/// Inserts an entity keyed by its `Name`, which the bundle must contain.
fn add<B: Bundle>(world: &mut AppendWorld, names: &mut UniqueIndex<String>, bundle: B) -> AppendId {
    world
        .insert_unique(names, bundle, |key| key.get::<Name>().0.clone())
        .id()
}

fn external_key(key: KeyView<'_, (ExternalId, Pos)>) -> ExternalId {
    *key.get::<ExternalId>()
}

fn name_key(key: KeyView<'_, (Name, Pos)>) -> String {
    key.get::<Name>().0.clone()
}

#[test]
fn ids_encode_the_location_and_are_dense_per_archetype() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (name("a"), pos(1.0)));
    let b = add(&mut world, &mut names, (name("b"), pos(2.0)));
    let c = add(
        &mut world,
        &mut names,
        (name("c"), pos(3.0), Vel { dx: 0.0, dy: 0.0 }),
    );

    assert_eq!((a.archetype().0, a.index()), (0, 0));
    assert_eq!((b.archetype().0, b.index()), (0, 1));
    assert_eq!((c.archetype().0, c.index()), (1, 0));
    assert_eq!(world.archetype_count(), 2);
    assert_eq!(world.len(), 3);

    assert_eq!(world.get::<Pos>(b), Some(&pos(2.0)));
    assert_eq!(world.get::<Pos>(c), Some(&pos(3.0)));
}

#[test]
fn same_layout_in_any_order_shares_an_archetype() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (pos(1.0), name("a")));
    let b = add(&mut world, &mut names, (name("b"), pos(2.0)));
    assert_eq!(a.archetype(), b.archetype());
    assert_eq!(world.get::<Name>(b), Some(&name("b")));
}

#[test]
fn lookups_of_missing_things_fail_cleanly() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (name("a"),));

    assert!(world.get::<Pos>(a).is_none());
    assert_eq!(world.get_mut::<Pos>(a), Err(MutError::Missing));

    let missing = AppendId::from_u64(0x0000_0007_0000_0003);
    assert!(!world.contains(missing));
    assert!(world.get::<Name>(missing).is_none());
    assert_eq!(world.get_mut::<Pos>(missing), Err(MutError::Missing));
}

#[test]
fn get_mut_writes_through() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (name("a"), pos(1.0)));
    world.get_mut::<Pos>(a).unwrap().x = 42.0;
    assert_eq!(world.get::<Pos>(a), Some(&pos(42.0)));
}

#[test]
fn ids_round_trip_through_u64() {
    let (mut world, mut names) = named_world();
    add(&mut world, &mut names, (name("a"),));
    let id = add(&mut world, &mut names, (name("b"), pos(1.0)));
    assert_eq!(AppendId::from_u64(id.to_u64()), id);
}

#[test]
fn iter_visits_everything_with_ids() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (name("a"),));
    let b = add(&mut world, &mut names, (name("b"), pos(0.0)));
    let c = add(&mut world, &mut names, (name("c"),));

    let mut seen: Vec<_> = world
        .iter::<Name>()
        .map(|(id, n)| (id, n.0.clone()))
        .collect();
    seen.sort();
    assert_eq!(seen, [(a, "a".into()), (c, "c".into()), (b, "b".into())]);
    assert_eq!(world.iter::<Vel>().count(), 0);
}

#[test]
fn add_global_keeps_ids_and_fills_existing_and_future_entities() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (name("a"),));
    let b = add(&mut world, &mut names, (name("b"), pos(1.0)));
    let before: Vec<_> = world.entities().collect();

    world.add_global(Color([7, 7, 7]));

    assert_eq!(world.entities().collect::<Vec<_>>(), before);
    assert_eq!(world.get::<Color>(a), Some(&Color([7, 7, 7])));
    assert_eq!(world.get::<Color>(b), Some(&Color([7, 7, 7])));
    assert_eq!(world.get::<Name>(a), Some(&name("a")));
    assert_eq!(world.get::<Pos>(b), Some(&pos(1.0)));

    // a later entity without a color gets the default and joins the extended archetype...
    let c = add(&mut world, &mut names, (name("c"),));
    assert_eq!(c.archetype(), a.archetype());
    assert_eq!(world.get::<Color>(c), Some(&Color([7, 7, 7])));
    // ...and one with an explicit color keeps it
    let d = add(&mut world, &mut names, (name("d"), Color([1, 2, 3])));
    assert_eq!(world.get::<Color>(d), Some(&Color([1, 2, 3])));
    assert_eq!(d.archetype(), a.archetype());
    // layouts stayed unique
    assert_eq!(world.archetype_count(), 2);
}

#[test]
#[should_panic(expected = "already global")]
fn a_component_can_only_be_global_once() {
    let mut world = AppendWorld::default();
    world.add_global(Color::default());
    world.add_global(Color::default());
}

#[test]
fn add_global_refuses_a_component_some_entity_already_has() {
    let (mut world, mut names) = named_world();
    let explicit = add(&mut world, &mut names, (name("a"), Color([1, 2, 3])));
    let plain = add(&mut world, &mut names, (name("b"),));

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| world.add_global(Color([9, 9, 9]))));
    assert!(result.is_err());

    // the failed call changed nothing: values are intact,
    // `plain` did not gain a color, and the component did not become global
    assert_eq!(world.get::<Color>(explicit), Some(&Color([1, 2, 3])));
    assert_eq!(world.get::<Color>(plain), None);
    let later = add(&mut world, &mut names, (name("c"),));
    assert_eq!(world.get::<Color>(later), None);
}

#[test]
fn for_each_computes_a_global_component_from_another() {
    let (mut world, mut names) = named_world();
    let a = add(&mut world, &mut names, (name("a"), pos(1.0)));
    let b = add(
        &mut world,
        &mut names,
        (name("b"), pos(2.0), Vel { dx: 0.0, dy: 0.0 }),
    );
    let no_pos = add(&mut world, &mut names, (name("c"),));
    world.add_global(Color::default());

    let mut visited = 0;
    world
        .for_each::<Pos, Color>(|_, pos, color| {
            visited += 1;
            *color = Color([pos.x as u8, 0, 0]);
        })
        .unwrap();

    assert_eq!(visited, 2);
    assert_eq!(world.get::<Color>(a), Some(&Color([1, 0, 0])));
    assert_eq!(world.get::<Color>(b), Some(&Color([2, 0, 0])));
    assert_eq!(world.get::<Color>(no_pos), Some(&Color::default()));
}

#[test]
#[should_panic(expected = "same component")]
fn for_each_rejects_aliasing() {
    let (mut world, mut names) = named_world();
    add(&mut world, &mut names, (name("a"), pos(1.0)));
    let _ = world.for_each::<Pos, Pos>(|_, _, _| {});
}

#[test]
fn external_ids_dedupe_and_have_a_reverse_lookup() {
    let mut world = AppendWorld::default();
    world.freeze::<ExternalId>();
    let mut index = UniqueIndex::<ExternalId>::new();

    let first = world.insert_unique(&mut index, (ExternalId(5), pos(1.0)), external_key);
    let second = world.insert_unique(&mut index, (ExternalId(5), pos(2.0)), external_key);
    let other = world.insert_unique(&mut index, (ExternalId(6), pos(3.0)), external_key);

    assert!(first.is_new() && other.is_new());
    assert_eq!(second, Inserted::Existing(first.id()));
    assert_eq!(world.len(), 2);
    assert_eq!(index.len(), 2);

    // forward through the index, reverse through an ordinary component
    assert_eq!(index.get(&ExternalId(6)), Some(other.id()));
    assert_eq!(world.get::<ExternalId>(other.id()), Some(&ExternalId(6)));
    // and the refused insert changed nothing
    assert_eq!(world.get::<Pos>(first.id()), Some(&pos(1.0)));
}

#[test]
fn dedup_can_look_at_only_some_of_the_components() {
    let (mut world, mut names) = named_world();

    let a = world.insert_unique(&mut names, (name("rock"), pos(1.0)), name_key);
    // same name, different position: still a duplicate
    let b = world.insert_unique(&mut names, (name("rock"), pos(99.0)), name_key);
    assert_eq!(b, Inserted::Existing(a.id()));
    assert_eq!(world.len(), 1);

    // lookups can use the borrowed form of the key
    assert_eq!(names.get("rock"), Some(a.id()));
}

#[test]
fn indexed_components_cannot_be_mutated() {
    let (mut world, mut names) = named_world();
    assert!(world.is_frozen::<Name>() && !world.is_frozen::<Pos>());
    let id = add(&mut world, &mut names, (name("rock"), pos(1.0)));
    let name_id = ComponentTypeId::of::<Name>();

    // neither mutation path gets through, and nothing changes
    assert_eq!(world.get_mut::<Name>(id), Err(MutError::Immutable(name_id)));
    assert_eq!(
        world.for_each::<Pos, Name>(|_, _, n| n.0.clear()),
        Err(MutError::Immutable(name_id))
    );
    assert_eq!(world.get::<Name>(id), Some(&name("rock")));
    assert_eq!(names.get("rock"), Some(id));

    // frozen components can still be read, including as the shared side of `for_each`...
    let mut seen = Vec::new();
    world
        .for_each::<Name, Pos>(|_, n, p| {
            seen.push(n.0.clone());
            p.x += 1.0;
        })
        .unwrap();
    assert_eq!(seen, ["rock"]);
    // ...and the other components stay mutable
    assert_eq!(world.get::<Pos>(id), Some(&pos(2.0)));
    world.get_mut::<Pos>(id).unwrap().x = 5.0;
    assert_eq!(world.get::<Pos>(id), Some(&pos(5.0)));
}

#[test]
#[should_panic(expected = "not immutable")]
fn a_key_cannot_read_a_component_that_is_not_frozen() {
    let mut world = AppendWorld::default();
    let mut names = UniqueIndex::<String>::new();
    world.insert_unique(&mut names, (name("a"), pos(0.0)), name_key);
}

#[test]
#[should_panic(expected = "does not contain")]
fn a_key_cannot_read_a_component_the_entity_lacks() {
    let mut world = AppendWorld::default();
    world.freeze::<ExternalId>();
    let mut index = UniqueIndex::<ExternalId>::new();
    world.insert_unique(&mut index, (name("a"),), |key| *key.get::<ExternalId>());
}

#[test]
fn components_are_dropped_exactly_once() {
    struct Tracked(#[allow(dead_code)] Arc<()>);

    fn tracked_key(key: KeyView<'_, (Name, Tracked)>) -> String {
        key.get::<Name>().0.clone()
    }

    let token = Arc::new(());
    {
        let (mut world, mut names) = named_world();

        world.insert_unique(
            &mut names,
            (name("a"), Tracked(Arc::clone(&token))),
            tracked_key,
        );
        assert_eq!(Arc::strong_count(&token), 2);

        // extending the layout of the archetype holding a `Tracked` must not drop or duplicate it
        world.add_global(Color::default());
        assert_eq!(Arc::strong_count(&token), 2);

        // a refused duplicate drops the bundle it was given
        world.insert_unique(
            &mut names,
            (name("b"), Tracked(Arc::clone(&token))),
            tracked_key,
        );
        assert_eq!(Arc::strong_count(&token), 3);
        let refused = world.insert_unique(
            &mut names,
            (name("b"), Tracked(Arc::clone(&token))),
            tracked_key,
        );
        assert!(!refused.is_new());
        assert_eq!(Arc::strong_count(&token), 3);
    }
    assert_eq!(Arc::strong_count(&token), 1);
}
