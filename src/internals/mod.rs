//! The implementation modules legion is built from.
//!
//! This module is only public when the `internals` feature is enabled, and exists for people who want
//! to build their own ECS out of legion's parts (entity ID allocation and location maps, archetypes
//! and layouts, component storages, query filters and views, the scheduler, ...).
//!
//! # Stability
//!
//! **Nothing here is covered by semver.** Paths, signatures, and behaviour may change in any release,
//! including patch releases, and items are not documented to the standard of the rest of the crate.
//! The curated modules ([`world`](crate::world), [`storage`](crate::storage), [`query`](crate::query),
//! [`systems`](crate::systems), ...) re-export the stable subset of these items.
//!
//! Items that legion itself needs to keep private (`pub(crate)`) stay private under this feature.

pub mod cons;
pub mod entity;
pub mod entry;
pub mod event;
pub mod hash;
pub mod insert;
pub mod iter;
pub mod permissions;
pub mod query;
#[cfg(feature = "serialize")]
pub mod serialize;
pub mod storage;
pub mod subworld;
pub mod systems;
pub mod world;
