use crate::{Val, error::RuntimeError};

/// Provides access to the arguments and runtime resources of a native call.
pub struct NativeContext<'a> {
    args: &'a [Val],
}

impl<'a> NativeContext<'a> {
    /// Creates a context with the evaluated arguments.
    pub fn new(args: &'a [Val]) -> Self {
        Self { args }
    }

    /// Returns the evaluated arguments supplied to this call.
    pub fn args(&self) -> &[Val] {
        self.args
    }
}

#[derive(Copy, Clone, Debug)]
/// A built-in function implemented in Rust.
pub struct NativeFunction {
    /// The name of the function.
    pub name: &'static str,
    /// The function called by the VM with the evaluated arguments.
    pub func: fn(NativeContext<'_>) -> Result<Val, RuntimeError>,
}

impl PartialEq for NativeFunction {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl NativeFunction {
    /// Evaluates this function with the supplied arguments.
    #[inline(always)]
    pub fn eval(&self, context: NativeContext<'_>) -> Result<Val, RuntimeError> {
        (self.func)(context)
    }
}

/// A Rust closure that can capture garbage-collected Lisp values.
pub struct NativeClosure {
    /// The name of the closure.
    pub name: String,
    implementation: Box<dyn NativeClosureImpl>,
}

trait NativeClosureImpl: 'static + Send {
    fn eval(&mut self, context: NativeContext<'_>) -> Result<Val, RuntimeError>;
}

impl NativeClosure {
    /// Creates a takolisp closure from a Rust closure.
    ///
    /// `eval` must not capture garbage-collected Lisp values.
    pub fn from_fn(
        name: impl Into<String>,
        eval: impl 'static + Send + FnMut(NativeContext<'_>) -> Result<Val, RuntimeError>,
    ) -> Self {
        Self {
            name: name.into(),
            implementation: Box::new(FnNativeClosure(eval)),
        }
    }

    /// Evaluates this closure with the supplied arguments.
    #[inline(always)]
    pub fn eval(&mut self, context: NativeContext<'_>) -> Result<Val, RuntimeError> {
        self.implementation.eval(context)
    }
}

struct FnNativeClosure<F>(F);

impl<F: 'static + Send + FnMut(NativeContext<'_>) -> Result<Val, RuntimeError>> NativeClosureImpl
    for FnNativeClosure<F>
{
    fn eval(&mut self, context: NativeContext<'_>) -> Result<Val, RuntimeError> {
        (self.0)(context)
    }
}

impl std::fmt::Debug for NativeClosure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeClosure")
            .field("name", &self.name.as_str())
            .finish_non_exhaustive()
    }
}
