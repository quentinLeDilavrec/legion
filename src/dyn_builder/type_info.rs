use std::alloc::Layout;

use crate::internals::storage::component::ComponentTypeId;

/// Metadata required to store a component.
///
/// All told, this means a [`TypeId`](std::any::TypeId), to be able to dynamically name/check the
/// component type; a [`Layout`], so that we know how to allocate memory for this component type;
/// and a drop function which internally calls [`core::ptr::drop_in_place`] with the correct type
/// parameter.
#[derive(Debug, Copy, Clone)]
pub struct TypeInfo {
    id: ComponentTypeId,
    layout: Layout,
    drop: unsafe fn(*mut u8),
}

impl TypeInfo {
    /// Construct a `TypeInfo` directly from the static type.
    pub fn of<T: 'static + Send + Sync>() -> Self {
        unsafe fn drop_ptr<T>(x: *mut u8) {
            unsafe { x.cast::<T>().drop_in_place() }
        }

        Self {
            id: ComponentTypeId::of::<T>(),
            layout: Layout::new::<T>(),
            drop: drop_ptr::<T>,
        }
    }

    /// Access the `TypeId` for this component type.
    pub fn id(&self) -> ComponentTypeId {
        self.id
    }

    /// Access the `Layout` of this component type.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Directly call the destructor on a pointer to data of this component type.
    ///
    /// # Safety
    ///
    /// All of the caveats of [`core::ptr::drop_in_place`] apply, with the additional requirement
    /// that this method is being called on a pointer to an object of the correct component type.
    pub unsafe fn drop(&self, data: *mut u8) {
        unsafe { (self.drop)(data) }
    }

    /// Get the function pointer encoding the destructor for the component type this `TypeInfo`
    /// represents.
    pub fn drop_shim(&self) -> unsafe fn(*mut u8) {
        self.drop
    }
}
