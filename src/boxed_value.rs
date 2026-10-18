use crate::val::Val;

/// A heap-allocated, mutable value.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BoxedValue {
    value: Val,
}

impl BoxedValue {
    /// Create a boxed value.
    pub fn from_value(value: Val) -> Self {
        Self { value }
    }

    /// Get the contained value.
    pub fn value(&self) -> Val {
        self.value
    }

    /// Get mutable access to the contained value.
    pub fn value_mut(&mut self) -> &mut Val {
        &mut self.value
    }
}
