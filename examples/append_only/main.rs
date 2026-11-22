//! An append-only world with location-encoded entity IDs, built from legion's `internals`.
//!
//! Run with `cargo run --example append_only --features internals`, and test with
//! `cargo test --example append_only --features internals`.
//!
//! The idea: if entities are never removed or moved between archetypes,
//! an entity's `(archetype, position)` is fixed for life,
//! so the ID can simply *be* that pair.
//! That removes the `ID -> location` map and the generation counters a recycling scheme needs.
//! Components can still be added to the whole world afterwards (`add_global`),
//! because that changes layouts but not positions.
//! Lookups by anything other than the ID (an external ID, a content-derived key)
//! go through secondary `UniqueIndex`es that live next to the world.
//! Those never go stale, because the components their keys come from are frozen: the world refuses to mutate them.

// This is an example API: the demo below does not call every method.
#![allow(dead_code)]

mod id;
mod index;
mod world;

#[cfg(test)]
mod tests;

use index::UniqueIndex;
use world::AppendWorld;

fn main() {
    let mut world = AppendWorld::default();

    // Components that feed an index key must be declared immutable up front.
    world.freeze::<ExternalId>();
    world.freeze::<Name>();

    let mut by_external = UniqueIndex::<ExternalId>::new();
    let mut by_name = UniqueIndex::<String>::new();

    // Intrinsic IDs: key entities by their name. Dedup looks at that component only, whatever the
    // other components are.
    let rock = world
        .insert_unique(
            &mut by_name,
            (Name("rock".into()), Pos { x: 1.0, y: 2.0 }),
            |key| key.get::<Name>().0.clone(),
        )
        .id();
    let again = world.insert_unique(
        &mut by_name,
        (Name("rock".into()), Pos { x: 100.0, y: 100.0 }),
        |key| key.get::<Name>().0.clone(),
    );
    println!("rock = {rock}; inserting another \"rock\" -> {again:?}");
    println!("world still has {} entities", world.len());

    // Entities with other component sets land in other archetypes.
    let tree = world
        .insert_unique(
            &mut by_name,
            (
                Name("tree".into()),
                Pos { x: 5.0, y: 1.0 },
                Vel { dx: 0.0, dy: 0.0 },
            ),
            |key| key.get::<Name>().0.clone(),
        )
        .id();
    println!("tree = {tree}  (archetype:position)");

    // Externally assigned IDs, indexed by that ID. The reverse lookup is a component read.
    let npc = world.insert_unique(
        &mut by_external,
        (ExternalId(9001), Name("npc".into()), Pos { x: 0.0, y: 0.0 }),
        |key| *key.get::<ExternalId>(),
    );
    let impostor = world.insert_unique(
        &mut by_external,
        (
            ExternalId(9001),
            Name("impostor".into()),
            Pos { x: 9.0, y: 9.0 },
        ),
        |key| *key.get::<ExternalId>(),
    );
    println!("npc = {npc:?}, same external ID again = {impostor:?}");
    println!("lookup 9001 -> {:?}", by_external.get(&ExternalId(9001)));
    println!(
        "external ID of {} = {:?}",
        npc.id(),
        world.get::<ExternalId>(npc.id())
    );

    // Indexed components cannot be changed behind the index's back; others can.
    println!(
        "mutate a name:     {:?}",
        world.get_mut::<Name>(rock).map(|_| ())
    );
    println!(
        "mutate a position: {:?}",
        world.get_mut::<Pos>(rock).map(|_| ())
    );

    // A component added to the whole world later. IDs do not change.
    let before: Vec<_> = world.entities().collect();
    world.add_global(Color([0, 0, 0]));
    assert_eq!(before, world.entities().collect::<Vec<_>>());

    // New entities get the default too, unless they bring their own value.
    let bush = world
        .insert_unique(
            &mut by_name,
            (Name("bush".into()), Pos { x: 3.0, y: 3.0 }),
            |key| key.get::<Name>().0.clone(),
        )
        .id();
    let flower = world
        .insert_unique(
            &mut by_name,
            (Name("flower".into()), Color([255, 0, 128])),
            |key| key.get::<Name>().0.clone(),
        )
        .id();

    // Compute the real values later, from other components.
    world
        .for_each::<Pos, Color>(|_, pos, color| {
            *color = Color([(pos.x * 10.0) as u8, (pos.y * 10.0) as u8, 0]);
        })
        .expect("Color is not frozen");

    for id in [rock, tree, npc.id(), bush, flower] {
        println!(
            "{id}: {:?} color = {:?}",
            world.get::<Name>(id).map(|n| n.0.as_str()),
            world.get::<Color>(id)
        );
    }
}

// Sample components used by the demo and the tests.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vel {
    pub dx: f32,
    pub dy: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Name(pub String);

/// An identifier assigned from outside the world (a network ID, a save-file ID, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExternalId(pub u64);

/// A component added to the whole world after the fact, with a default value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color(pub [u8; 3]);
