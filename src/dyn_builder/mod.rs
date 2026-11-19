//! Dynamically build entities whose component types are only known at runtime.
//!
//! An [`EntityBuilder`] collects component values one at a time (via
//! [`EntityBuilder::add`]) and can then be passed to
//! [`World::extend`](crate::world::World::extend) to insert it as a single entity.
//!
//! ```
//! # use legion::*;
//! # use legion::dyn_builder::EntityBuilder;
//! let mut world = World::default();
//! let mut builder = EntityBuilder::default();
//! builder.add(42i32).add(true);
//! let entity = world.extend(builder)[0];
//! assert_eq!(world.entry(entity).unwrap().get_component::<i32>(), Ok(&42));
//! ```
//!
//! Note the idea came from mixing hecs entity builder with IntoComponentSource to dynamically build entities.

mod builder;
mod source;
mod type_id_map;
mod type_info;

pub use builder::EntityBuilder;
pub use type_info::TypeInfo;

#[cfg(test)]
mod tests;
