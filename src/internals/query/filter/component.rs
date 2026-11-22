use std::marker::PhantomData;

use super::{
    and::And, not::Not, or::Or, passthrough::Passthrough, ActiveFilter, FilterResult, GroupMatcher,
    LayoutFilter,
};
use crate::internals::storage::component::ComponentTypeId;

/// A filter which matches `true` when the given component exists in the archetype.
#[derive(Debug)]
pub struct ComponentFilter<T: 'static>(PhantomData<T>);

impl<T: 'static> Default for ComponentFilter<T> {
    fn default() -> Self {
        ComponentFilter(PhantomData)
    }
}

impl<T: 'static> Copy for ComponentFilter<T> {}
impl<T: 'static> Clone for ComponentFilter<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> ActiveFilter for ComponentFilter<T> {}

impl<T: 'static> GroupMatcher for ComponentFilter<T> {
    fn can_match_group() -> bool {
        true
    }
    fn group_components() -> Vec<ComponentTypeId> {
        vec![ComponentTypeId::of::<T>()]
    }
}

impl<T: 'static> LayoutFilter for ComponentFilter<T> {
    fn matches_layout(&self, components: &[ComponentTypeId]) -> FilterResult {
        FilterResult::Match(components.contains(&ComponentTypeId::of::<T>()))
    }
}

impl<T: 'static> std::ops::Not for ComponentFilter<T> {
    type Output = Not<Self>;

    #[inline]
    fn not(self) -> Self::Output {
        Not { filter: self }
    }
}

impl<T: 'static, Rhs: ActiveFilter> std::ops::BitAnd<Rhs> for ComponentFilter<T> {
    type Output = And<(Self, Rhs)>;

    #[inline]
    fn bitand(self, rhs: Rhs) -> Self::Output {
        And {
            filters: (self, rhs),
        }
    }
}

impl<T: 'static> std::ops::BitAnd<Passthrough> for ComponentFilter<T> {
    type Output = Self;

    #[inline]
    fn bitand(self, _: Passthrough) -> Self::Output {
        self
    }
}

impl<T, Rhs> std::ops::BitOr<Rhs> for ComponentFilter<T>
where
    T: 'static,
    Rhs: ActiveFilter,
{
    type Output = Or<(Self, Rhs)>;

    #[inline]
    fn bitor(self, rhs: Rhs) -> Self::Output {
        Or {
            filters: (self, rhs),
        }
    }
}

impl<T: 'static> std::ops::BitOr<Passthrough> for ComponentFilter<T> {
    type Output = Self;

    #[inline]
    fn bitor(self, _: Passthrough) -> Self::Output {
        self
    }
}
