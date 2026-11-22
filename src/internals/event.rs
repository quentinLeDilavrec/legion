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
///
/// `Id` is the type identifying entities, which is [`Entity`] unless a custom identifier scheme
/// is in use.
#[derive(Debug, Clone)]
pub enum Event<Id = Entity> {
    /// A new archetype has been created.
    ArchetypeCreated(ArchetypeIndex),
    /// An entity has been inserted into an archetype.
    EntityInserted(Id, ArchetypeIndex),
    /// An entity has been removed from an archetype.
    EntityRemoved(Id, ArchetypeIndex),
}

/// Describes a type which can send entity events.
pub trait EventSender<Id = Entity>: Send + Sync {
    /// Sends the given event to all listeners.
    /// Returns `true` if the sender is still alive.
    fn send(&self, event: Event<Id>) -> bool;
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
impl<Id, F> EventSender<Id> for F
where
    F: Fn(Event<Id>) -> bool + Send + Sync,
{
    fn send(&self, event: Event<Id>) -> bool {
        self(event)
    }
}

/// Allows a sender to be shared, e.g. between several subscriptions.
impl<Id, T: EventSender<Id> + ?Sized> EventSender<Id> for Arc<T> {
    fn send(&self, event: Event<Id>) -> bool {
        (**self).send(event)
    }
}

/// Collects all events into a vector. Wrap in an [`Arc`] to keep a handle for reading them.
impl<Id: Send> EventSender<Id> for Mutex<Vec<Event<Id>>> {
    fn send(&self, event: Event<Id>) -> bool {
        self.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        true
    }
}

impl<Id: Send> EventSender<Id> for std::sync::mpsc::Sender<Event<Id>> {
    fn send(&self, event: Event<Id>) -> bool {
        std::sync::mpsc::Sender::send(self, event).is_ok()
    }
}

/// Events are dropped if the channel is full, but the subscription stays alive.
impl<Id: Send> EventSender<Id> for std::sync::mpsc::SyncSender<Event<Id>> {
    fn send(&self, event: Event<Id>) -> bool {
        !matches!(
            self.try_send(event),
            Err(std::sync::mpsc::TrySendError::Disconnected(_))
        )
    }
}

/// A listener for archetype and entity events: an event sender, plus a layout filter selecting
/// the archetypes it is interested in.
pub struct Subscriber<Id = Entity> {
    filter: Arc<dyn LayoutFilter + Send + Sync>,
    sender: Arc<dyn EventSender<Id>>,
}

// Implemented by hand so that cloning does not require `Id: Clone`.
impl<Id> Clone for Subscriber<Id> {
    fn clone(&self) -> Self {
        Self {
            filter: Arc::clone(&self.filter),
            sender: Arc::clone(&self.sender),
        }
    }
}

impl<Id> Subscriber<Id> {
    /// Constructs a subscriber which sends events about archetypes matching `filter` to `sender`.
    ///
    /// The filter is applied to an archetype's component types, not to its entities.
    pub fn new<F, S>(filter: F, sender: S) -> Self
    where
        F: LayoutFilter + Send + Sync + 'static,
        S: EventSender<Id> + 'static,
    {
        Self {
            filter: Arc::new(filter),
            sender: Arc::new(sender),
        }
    }

    /// Returns `true` if the subscriber's filter matches the archetype's layout.
    pub fn is_interested(&self, archetype: &Archetype<Id>) -> bool
    where
        Id: Copy,
    {
        self.filter
            .matches_layout(archetype.layout().component_types())
            .is_pass()
    }

    pub(crate) fn send(&self, message: Event<Id>) -> bool {
        self.sender.send(message)
    }
}

/// The set of [`Subscriber`]s listening to a single archetype.
///
/// Subscribers whose sender reports it is no longer alive are dropped the next time an event is
/// sent. [`Archetype::new`](super::storage::archetype::Archetype::new) takes one of these.
/// Use `Subscribers::default()` for an archetype nobody listens to.
pub struct Subscribers<Id = Entity> {
    subscribers: Vec<Subscriber<Id>>,
}

// Implemented by hand so that these do not require `Id: Clone` or `Id: Default`.
impl<Id> Clone for Subscribers<Id> {
    fn clone(&self) -> Self {
        Self {
            subscribers: self.subscribers.clone(),
        }
    }
}

impl<Id> Default for Subscribers<Id> {
    fn default() -> Self {
        Self {
            subscribers: Vec::new(),
        }
    }
}

impl<Id> Subscribers<Id> {
    /// Adds a subscriber.
    pub fn push(&mut self, subscriber: Subscriber<Id>) {
        self.subscribers.push(subscriber);
    }

    /// Sends an event to every subscriber, dropping the ones which are no longer alive.
    pub fn send(&mut self, message: Event<Id>)
    where
        Id: Clone,
    {
        for i in (0..self.subscribers.len()).rev() {
            if !self.subscribers[i].send(message.clone()) {
                self.subscribers.swap_remove(i);
            }
        }
    }

    /// Returns the subscribers whose filter matches the given component types.
    ///
    /// This is how the subscribers of a world are narrowed down to the ones an archetype with
    /// that layout should notify.
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

impl<Id> Debug for Subscribers<Id> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscribers")
            .field("len", &self.subscribers.len())
            .finish()
    }
}
