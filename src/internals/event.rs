use std::{
    fmt::Debug,
    iter::Iterator,
    sync::{Arc, Mutex, PoisonError},
};

use super::{
    entity::Entity,
    query::filter::LayoutFilter,
    storage::{
        archetype::{Archetype, ArchetypeIndex},
        component::ComponentTypeId,
    },
};

/// Events emitted by a world to subscribers. See `World.subscribe(Sender, EntityFilter)`.
#[derive(Debug, Clone)]
pub enum Event {
    /// A new archetype has been created.
    ArchetypeCreated(ArchetypeIndex),
    /// An entity has been inserted into an archetype.
    EntityInserted(Entity, ArchetypeIndex),
    /// An entity has been removed from an archetype.
    EntityRemoved(Entity, ArchetypeIndex),
}

/// Describes a type which can send entity events.
pub trait EventSender: Send + Sync {
    /// Sends the given event to all listeners.
    /// Returns `true` if the sender is still alive.
    fn send(&self, event: Event) -> bool;
}

/// Any closure can act as an event sender, which allows adapting arbitrary channel types.
/// The closure should return `true` while the subscription should stay alive.
///
/// ```
/// # use legion::*;
/// # let mut world = World::default();
/// let (tx, rx) = std::sync::mpsc::channel::<legion::world::Event>();
/// world.subscribe(move |event| tx.send(event).is_ok(), any());
/// ```
impl<F> EventSender for F
where
    F: Fn(Event) -> bool + Send + Sync,
{
    fn send(&self, event: Event) -> bool {
        self(event)
    }
}

/// Allows a sender to be shared, e.g. between several subscriptions.
impl<T: EventSender + ?Sized> EventSender for Arc<T> {
    fn send(&self, event: Event) -> bool {
        (**self).send(event)
    }
}

/// Collects all events into a vector. Wrap in an [`Arc`] to keep a handle for reading them.
impl EventSender for Mutex<Vec<Event>> {
    fn send(&self, event: Event) -> bool {
        self.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        true
    }
}

impl EventSender for std::sync::mpsc::Sender<Event> {
    fn send(&self, event: Event) -> bool {
        std::sync::mpsc::Sender::send(self, event).is_ok()
    }
}

/// Events are dropped if the channel is full, but the subscription stays alive.
impl EventSender for std::sync::mpsc::SyncSender<Event> {
    fn send(&self, event: Event) -> bool {
        !matches!(
            self.try_send(event),
            Err(std::sync::mpsc::TrySendError::Disconnected(_))
        )
    }
}

#[derive(Clone)]
pub(crate) struct Subscriber {
    filter: Arc<dyn LayoutFilter + Send + Sync>,
    sender: Arc<dyn EventSender>,
}

impl Subscriber {
    pub(crate) fn new<F, S>(filter: F, sender: S) -> Self
    where
        F: LayoutFilter + Send + Sync + 'static,
        S: EventSender + 'static,
    {
        Self {
            filter: Arc::new(filter),
            sender: Arc::new(sender),
        }
    }

    pub(crate) fn is_interested(&self, archetype: &Archetype) -> bool {
        self.filter
            .matches_layout(archetype.layout().component_types())
            .is_pass()
    }

    pub(crate) fn send(&self, message: Event) -> bool {
        self.sender.send(message)
    }
}

#[derive(Clone, Default)]
pub(crate) struct Subscribers {
    subscribers: Vec<Subscriber>,
}

impl Subscribers {
    pub fn push(&mut self, subscriber: Subscriber) {
        self.subscribers.push(subscriber);
    }

    pub fn send(&mut self, message: Event) {
        for i in (0..self.subscribers.len()).rev() {
            if !self.subscribers[i].send(message.clone()) {
                self.subscribers.swap_remove(i);
            }
        }
    }

    pub fn matches_layout(&self, components: &[ComponentTypeId]) -> Self {
        Self {
            subscribers: self
                .subscribers
                .iter()
                .filter(|sub| sub.filter.matches_layout(components).is_pass())
                .cloned()
                .collect(),
        }
    }
}

impl Debug for Subscribers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscribers")
            .field("len", &self.subscribers.len())
            .finish()
    }
}
