use crate::val::Val;

/// Holds two values
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Pair {
    pub car: Val,
    pub cdr: Val,
}
