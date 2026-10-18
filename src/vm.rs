use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use bumpalo::Bump;
use smallvec::SmallVec;

use crate::gc::MarkAndSweepGarbageCollector;
use crate::{
    ast::Ast,
    boxed_value::BoxedValue,
    builtins::register_builtins,
    error::{RuntimeError, VmError},
    function::{
        BytecodeFunction, CaptureSource, CaptureStorage, ClosureCaptures, Instruction, UpvalueSlot,
    },
    heap::{Heap, HeapHandle},
    module::{Module, ModuleSource},
    native_function::NativeContext,
    source_map::SourceMap,
    symbol::Symbol,
    val::Val,
};

/// A virtual machine that evaluates takolisp values and bytecode.
///
/// A new VM includes the standard built-in functions. Globals can be added with
/// [`Vm::set_global`]. The global module is immutable from the perspective of
/// compiled code: the compiler inlines global values as constants, so globals
/// must be added before compiling code that references them.
pub struct Vm {
    /// Owns every heap-allocated value used by the VM: modules, symbols, strings,
    /// lists, bytecode functions, boxed values, and closure captures.
    heap: Heap,
    /// Source text compiled by this VM, indexed by
    /// [`SourceId`](crate::source_map::SourceId) so that errors can be reported
    /// against the code that produced them.
    pub(crate) source_map: SourceMap,
    /// Module holding global values. All modules survive garbage collection.
    global_module: HeapHandle<Module>,
    /// Empty module used as a placeholder for call frames that have no module of
    /// their own, such as frames recorded for failed native function calls. It is
    /// retained with the other modules, so the placeholder handle always resolves.
    null_module: HeapHandle<Module>,
    /// Mark sets reused by each [`Vm::collect_garbage`] pass.
    garbage_collector: MarkAndSweepGarbageCollector,
    /// Value stack holding operands, function arguments, locals, and temporaries
    /// for the active call frames. Call frames own contiguous slices of it.
    stack: Vec<Val>,
    /// Active call frames, innermost last, each executing bytecode whose operands
    /// and locals live in `stack`.
    pub(crate) call_frames: CallFrames,
}

impl Default for Vm {
    fn default() -> Self {
        let mut heap = Heap::default();
        let global_module = heap
            .modules
            .insert(ModuleSource::Global)
            .expect("global module source is unique");
        let null_module = heap
            .modules
            .insert(ModuleSource::Internal(""))
            .expect("null module source is unique");
        Self {
            heap,
            source_map: SourceMap::default(),
            global_module,
            null_module,
            garbage_collector: MarkAndSweepGarbageCollector::default(),
            stack: Vec::with_capacity(1024),
            call_frames: CallFrames::default(),
        }
    }
}

impl Vm {
    /// Create a VM with the standard built-in functions registered.
    pub fn new() -> Vm {
        let mut vm = Vm::default();
        register_builtins(&mut vm);
        vm
    }

    /// Set or replace a global value.
    ///
    /// The global module is treated as immutable by the compiler: values are
    /// inlined as constants when code is compiled. Set all globals before
    /// compiling code that references them.
    ///
    /// # Examples
    ///
    /// ```
    /// use takolisp::{symbol::Symbol, Val, Vm};
    ///
    /// let mut vm = Vm::new();
    /// vm.set_global(Symbol::must_inline("answer"), Val::Int(42));
    /// assert_eq!(vm.eval_str(vm.global_module(), "answer"), Ok(Val::Int(42)));
    /// ```
    pub fn set_global(&mut self, symbol: Symbol, val: Val) {
        let Some(module) = self.heap.modules.get_mut(self.global_module) else {
            // Modules are retained by collection. Drop the write instead of
            // panicking if an embedder removes this module from the heap.
            debug_assert!(
                false,
                "the global module is missing from the heap: {symbol:?} was not set"
            );
            return;
        };
        module.set(symbol, val);
    }

    /// Returns the value of a global, or [`Val::Undefined`] when the symbol has
    /// not been defined. Use [`Val::is_undefined`] to test the result.
    ///
    /// A global module that an embedder removed from the heap reads the same as
    /// an empty one, since garbage collection never reclaims it.
    ///
    /// # Examples
    ///
    /// ```
    /// use takolisp::{symbol::Symbol, Val, Vm};
    ///
    /// let mut vm = Vm::new();
    /// vm.set_global(Symbol::must_inline("answer"), Val::Int(42));
    /// assert_eq!(vm.get_global(Symbol::must_inline("answer")), Val::Int(42));
    /// assert_eq!(
    ///     vm.get_global(Symbol::must_inline("missing")),
    ///     Val::Undefined
    /// );
    /// ```
    pub fn get_global(&self, symbol: Symbol) -> Val {
        self.heap
            .modules
            .get(self.global_module)
            .map(|module| module.get(symbol))
            .unwrap_or(Val::Undefined)
    }

    /// Get access to the VM's heap.
    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    /// Get mutable access to the VM's heap.
    pub fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// Get a handle to the VM's global module.
    pub fn global_module(&self) -> HeapHandle<Module> {
        self.global_module
    }

    /// Create an empty module with the given source.
    /// The module and its globals survive garbage collection.
    /// Returns an error if a module with this source already exists.
    pub fn create_root_module(
        &mut self,
        source: ModuleSource,
    ) -> Result<HeapHandle<Module>, crate::heap::DuplicateModuleSource> {
        self.heap.modules.insert(source)
    }

    /// Collect heap values unreachable from modules and the active evaluation.
    pub fn collect_garbage(&mut self) {
        self.garbage_collector.clear();
        for handle in self.heap.modules.handles() {
            self.garbage_collector.visit_module(&self.heap, handle);
        }
        self.garbage_collector
            .visit(&self.heap, self.stack.iter().copied());
        self.garbage_collector
            .visit(&self.heap, self.call_frames.iter_functions());
        self.garbage_collector.finalize(&mut self.heap);
    }

    /// Parse and evaluate source in `module`, with VM globals inlined as
    /// constants at compile time. Modules survive garbage collection.
    ///
    /// # Examples
    ///
    /// ```
    /// use takolisp::{ModuleSource, Val, Vm};
    ///
    /// let mut vm = Vm::new();
    /// let module = vm.create_root_module(ModuleSource::Internal("test")).unwrap();
    /// assert_eq!(vm.eval_str(module, "(+ 1 2)"), Ok(Val::Int(3)));
    /// ```
    pub fn eval_str(&mut self, module: HeapHandle<Module>, source: &str) -> Result<Val, VmError> {
        let func = {
            let bump = Bump::new();
            let source_id = self.source_map.add_source(source);
            let ast =
                Ast::parse(source_id, source, &bump, &mut self.heap).map_err(VmError::from)?;
            ast.compile(self, module)
                .map_err(|error| VmError::from_compiler(error, source_id))?
        };
        let result = self.eval(func, std::iter::empty()).map_err(VmError::from)?;
        Ok(result)
    }

    /// Load and evaluate a file once, returning its module on subsequent calls.
    ///
    /// A failed load can be retried after the file is fixed.
    pub fn ensure_loaded(&mut self, path: &Path) -> Result<HeapHandle<Module>, VmError> {
        let module_source = ModuleSource::File(path.to_owned());
        let module_handle = match self.heap.modules.insert(module_source) {
            Ok(handle) => handle,
            Err(duplicate) => return Ok(duplicate.existing),
        };
        let bump = Bump::new();
        let result = (|| {
            let source = std::fs::read_to_string(path)
                .map_err(|error| VmError::from_file_read(path, error))?;
            let source_id = self.source_map.add_source(source.clone());
            let ast =
                Ast::parse(source_id, &source, &bump, &mut self.heap).map_err(VmError::from)?;
            let func = ast
                .compile(self, module_handle)
                .map_err(|error| VmError::from_compiler(error, source_id))?;
            self.eval(func, std::iter::empty()).map_err(VmError::from)?;
            Ok(module_handle)
        })();
        if result.is_err() {
            self.heap.modules.remove(module_handle);
        }
        result
    }

    /// Evaluate a function value with the supplied arguments.
    pub fn eval(
        &mut self,
        func_val: Val,
        args: impl IntoIterator<Item = Val>,
    ) -> Result<Val, RuntimeError> {
        self.stack.clear();
        self.call_frames.reset();
        self.stack.extend(std::iter::once(func_val).chain(args));
        let (args_start, arg_count) = (1, self.stack.len() - 1);
        match func_val {
            Val::NativeFunction(native_function) => native_function
                .eval(NativeContext::new(&self.stack[args_start..]))
                .inspect_err(|_| {
                    std::hint::cold_path();
                    self.call_frames.push_synthetic(
                        self.null_module,
                        func_val,
                        args_start..self.stack.len(),
                    );
                }),
            Val::NativeClosure(handle) => {
                let native_closure = self
                    .heap
                    .native_closures
                    .get_mut(handle)
                    .ok_or(RuntimeError::InvalidVmState("native closure was collected"))?;
                native_closure
                    .eval(NativeContext::new(&self.stack[args_start..]))
                    .inspect_err(|_| {
                        std::hint::cold_path();
                        self.call_frames.push_synthetic(
                            self.null_module,
                            func_val,
                            args_start..self.stack.len(),
                        );
                    })
            }
            Val::BytecodeFunction(bytecode_handle) => {
                self.run(bytecode_handle, None, args_start, arg_count)
            }
            Val::Closure(closure_handle) => {
                let bytecode_handle = self
                    .heap
                    .closures
                    .get(closure_handle)
                    .ok_or(RuntimeError::InvalidVmState("closure was collected"))?
                    .function;
                self.run(bytecode_handle, Some(closure_handle), args_start, arg_count)
            }
            _ => {
                std::hint::cold_path();
                Err(RuntimeError::TypeError {
                    expected: "function",
                    found: func_val,
                })
            }
        }
    }

    fn run(
        &mut self,
        bytecode_handle: HeapHandle<BytecodeFunction>,
        closure_handle: Option<HeapHandle<ClosureCaptures>>,
        args_start: usize,
        arg_count: usize,
    ) -> Result<Val, RuntimeError> {
        self.call_frames.push_bytecode_frame(
            &mut self.stack,
            &self.heap,
            bytecode_handle,
            closure_handle,
            args_start,
            arg_count,
        )?;
        let result = self.run_inner();
        if result.is_err() {
            // Abandoned call frames never execute `Return`, so box their open
            // upvalues here. Otherwise closures that escaped the frame would keep
            // a stack capture pointing at slots that no longer exist.
            self.call_frames.close_upvalues(&self.stack, &mut self.heap);
        }
        result
    }

    fn run_inner(&mut self) -> Result<Val, RuntimeError> {
        // TODO: It's harder than it seems to break up this function due to the `call_frame`
        // reference.
        let mut call_frame = self
            .call_frames
            .current_mut()
            .ok_or(RuntimeError::StackUnderflow)?;
        loop {
            let instruction = call_frame
                .instructions
                .get(call_frame.instruction_idx)
                .ok_or(RuntimeError::InvalidBytecode(
                    "instruction pointer ran past the end of the bytecode",
                ))?;
            call_frame.instruction_idx += 1;
            match instruction {
                Instruction::PushConst(val) => self.stack.push(*val),
                Instruction::LookupSlot(slot) => {
                    let module_handle = call_frame.module;
                    let module = self
                        .heap
                        .modules
                        .get(module_handle)
                        .ok_or(RuntimeError::InvalidVmState("module was collected"))?;
                    let val = module.get_slot(*slot);
                    if val.is_undefined() {
                        std::hint::cold_path();
                        return Err(RuntimeError::ValueNotFound(module.symbol(*slot)));
                    }
                    self.stack.push(val);
                }
                Instruction::MakeClosure(bytecode_handle) => {
                    let bytecode_handle = *bytecode_handle;
                    let closure_handle = self
                        .call_frames
                        .make_closure(&mut self.heap, bytecode_handle)?;
                    self.stack.push(Val::Closure(closure_handle));
                    // Restore `call_frame` since we borrowed self.call_frames as mutable.
                    call_frame = self
                        .call_frames
                        .current_mut()
                        .ok_or(RuntimeError::InvalidVmState("CallFrame lost in closure"))?;
                }
                Instruction::GetUpvalue(slot) => {
                    let val = call_frame.upvalue(*slot, &self.heap, &self.stack)?;
                    self.stack.push(val);
                }
                Instruction::SetUpvalue(slot) => {
                    let val = self.stack.pop().ok_or(RuntimeError::StackUnderflow)?;
                    call_frame.set_upvalue(*slot, val, &mut self.heap, &mut self.stack)?;
                    self.stack.push(Val::Undefined);
                }
                Instruction::GetLocal(index) => {
                    let absolute_index = call_frame.start_address + index;
                    let val =
                        *self
                            .stack
                            .get(absolute_index)
                            .ok_or(RuntimeError::InvalidVmState(
                                "local slot is outside the stack",
                            ))?;
                    self.stack.push(val);
                }
                Instruction::GetCurrentFunction => {
                    let val = call_frame.function;
                    self.stack.push(val);
                }
                Instruction::SetLocal(index) => {
                    let absolute_index = call_frame.start_address + index;
                    self.stack
                        .get(absolute_index)
                        .ok_or(RuntimeError::InvalidVmState(
                            "local slot is outside the stack",
                        ))?;
                    let val = self.stack.pop().ok_or(RuntimeError::StackUnderflow)?;
                    self.stack[absolute_index] = val;
                }
                Instruction::Define(slot) => {
                    let module_handle = call_frame.module;
                    let module = self
                        .heap
                        .modules
                        .get_mut(module_handle)
                        .ok_or(RuntimeError::InvalidVmState("module was collected"))?;
                    let top = self.stack.last_mut().ok_or(RuntimeError::StackUnderflow)?;
                    let val = std::mem::replace(top, Val::Undefined);
                    if !module.set_slot(*slot, val) {
                        std::hint::cold_path();
                        return Err(RuntimeError::InvalidVmState(
                            "define slot is outside the module",
                        ));
                    }
                }
                Instruction::Eval(n) => {
                    let n = *n;
                    let required_stack_len = n.checked_add(1).ok_or(RuntimeError::StackOverflow)?;
                    if self
                        .stack
                        .len()
                        .saturating_sub(call_frame.operand_stack_start)
                        < required_stack_len
                    {
                        return Err(RuntimeError::StackUnderflow);
                    }
                    let args_start = self.stack.len() - n;
                    let func_idx = args_start - 1;
                    let func_val = &self.stack[func_idx];
                    match func_val {
                        Val::NativeFunction(native_function) => {
                            let val_result =
                                native_function.eval(NativeContext::new(&self.stack[args_start..]));
                            match val_result {
                                Ok(val) => {
                                    self.stack[func_idx] = val;
                                    self.stack.truncate(func_idx + 1);
                                }
                                Err(error) => {
                                    std::hint::cold_path();
                                    self.call_frames.push_synthetic(
                                        self.null_module,
                                        *func_val,
                                        args_start..self.stack.len(),
                                    );
                                    return Err(error);
                                }
                            };
                        }
                        Val::NativeClosure(handle) => {
                            let native_closure = self.heap.native_closures.get_mut(*handle).ok_or(
                                RuntimeError::InvalidVmState("native closure was collected"),
                            )?;
                            let val_result =
                                native_closure.eval(NativeContext::new(&self.stack[args_start..]));
                            match val_result {
                                Ok(val) => {
                                    self.stack[func_idx] = val;
                                    self.stack.truncate(func_idx + 1);
                                }
                                Err(error) => {
                                    std::hint::cold_path();
                                    self.call_frames.push_synthetic(
                                        self.null_module,
                                        *func_val,
                                        args_start..self.stack.len(),
                                    );
                                    return Err(error);
                                }
                            };
                        }
                        Val::BytecodeFunction(bytecode_handle) => {
                            let bytecode_handle = *bytecode_handle;
                            call_frame = self.call_frames.push_bytecode_frame(
                                &mut self.stack,
                                &self.heap,
                                bytecode_handle,
                                None,
                                args_start,
                                n,
                            )?;
                        }
                        Val::Closure(closure_handle) => {
                            let bytecode_handle = self
                                .heap
                                .closures
                                .get(*closure_handle)
                                .ok_or(RuntimeError::InvalidVmState("closure was collected"))?
                                .function;
                            let closure_handle = *closure_handle;
                            call_frame = self.call_frames.push_bytecode_frame(
                                &mut self.stack,
                                &self.heap,
                                bytecode_handle,
                                Some(closure_handle),
                                args_start,
                                n,
                            )?;
                        }
                        _ => {
                            std::hint::cold_path();
                            return Err(RuntimeError::TypeError {
                                expected: "function",
                                found: *func_val,
                            });
                        }
                    }
                }
                Instruction::PopN(n) => {
                    if *n
                        > self
                            .stack
                            .len()
                            .saturating_sub(call_frame.operand_stack_start)
                    {
                        std::hint::cold_path();
                        return Err(RuntimeError::StackUnderflow);
                    }
                    self.stack.truncate(self.stack.len() - n);
                }
                Instruction::JumpIfFalse(target) => {
                    let condition = self.stack.pop().ok_or(RuntimeError::StackUnderflow)?;
                    if condition == Val::Bool(false) {
                        call_frame.instruction_idx = *target;
                    }
                }
                Instruction::Jump(target) => call_frame.instruction_idx = *target,
                Instruction::Return => {
                    let return_result = self
                        .call_frames
                        .return_value(&mut self.stack, &mut self.heap)?;
                    match return_result {
                        ValOrCallFrame::Val(val) => return Ok(val),
                        ValOrCallFrame::CallFrame(f) => {
                            call_frame = f;
                        }
                    }
                }
            }
        }
    }
}

pub(crate) struct CallFrames(Vec<CallFrame>);

#[derive(Debug)]
pub(crate) struct CallFrame {
    /// The module whose values are used by this frame.
    module: HeapHandle<Module>,
    /// The instructions being executed.
    ///
    /// Shared with the bytecode function on the heap so that the VM can execute without borrowing
    /// the heap.
    instructions: Arc<[Instruction]>,
    /// The function or closure being called, retained for backtraces and GC.
    pub(crate) function: Val,
    /// The captures of the called closure, or `None` for a non-closure function.
    pub(crate) closure_values: Option<HeapHandle<ClosureCaptures>>,
    /// Closure slots that capture this frame's stack values and must be boxed
    /// before those values are removed from the stack.
    open_upvalues: SmallVec<[OpenUpvalue; 2]>,
    /// The index of the next instruction to execute in `instructions`.
    instruction_idx: usize,
    /// The stack index of the called function, replaced by the return value
    /// when this frame returns.
    return_address: usize,
    /// The stack index of the first argument, used as the base for local slots.
    start_address: usize,
    /// The stack index where operands begin, after the arguments and locals.
    operand_stack_start: usize,
}

/// A reference to a closure slot that currently points into the VM stack.
///
/// When the owning call frame returns, this slot can be changed to point at a
/// boxed value instead. Keeping the closure and slot indices here means that
/// closing an upvalue does not require scanning every closure in the heap.
#[derive(Debug)]
struct OpenUpvalue {
    stack_index: usize,
    closure_values: HeapHandle<ClosureCaptures>,
    closure_index: usize,
}

/// Captures that point into the VM stack, as `(closure_index, stack_index)`
/// pairs, which must be opened with the call frames that own their slots.
type StackCaptures = SmallVec<[(usize, usize); 2]>;

impl Default for CallFrames {
    fn default() -> CallFrames {
        CallFrames(Vec::with_capacity(32))
    }
}

enum ValOrCallFrame<'a> {
    Val(Val),
    CallFrame(&'a mut CallFrame),
}

impl CallFrames {
    pub(crate) fn reset(&mut self) {
        self.0.clear();
    }

    pub(crate) fn iter_functions(&self) -> impl Iterator<Item = Val> {
        self.0.iter().map(|frame| frame.function)
    }

    fn close_upvalues(&mut self, stack: &[Val], heap: &mut Heap) {
        for frame in self.0.iter_mut() {
            close_upvalues(&frame.open_upvalues, stack, heap);
        }
    }

    pub(crate) fn current_mut(&mut self) -> Option<&mut CallFrame> {
        self.0.last_mut()
    }

    pub(crate) fn current(&self) -> Option<&CallFrame> {
        self.0.last()
    }

    pub(crate) fn backtrace(&self) -> impl '_ + Iterator<Item = &CallFrame> {
        self.0.iter().rev()
    }

    /// Creates the closure for `bytecode_handle` in the current call frame and
    /// opens its stack captures with the frames that own those slots, so that
    /// the captures are boxed when those frames return.
    ///
    /// Returns the handle of the new closure, which the caller pushes as a value.
    fn make_closure(
        &mut self,
        heap: &mut Heap,
        bytecode_handle: HeapHandle<BytecodeFunction>,
    ) -> Result<HeapHandle<ClosureCaptures>, RuntimeError> {
        let (closure_handle, stack_captures) = self
            .0
            .last()
            .ok_or(RuntimeError::InvalidVmState(
                "cannot make a closure without a call frame",
            ))?
            .build_captures(heap, bytecode_handle)?;
        self.open_upvalues(closure_handle, stack_captures)?;
        Ok(closure_handle)
    }

    /// Opens `stack_captures` with the call frames that own their stack slots,
    /// so that the captures are boxed when those frames return.
    fn open_upvalues(
        &mut self,
        closure_values: HeapHandle<ClosureCaptures>,
        stack_captures: StackCaptures,
    ) -> Result<(), RuntimeError> {
        for (closure_index, stack_index) in stack_captures {
            let owner = self
                .0
                .iter_mut()
                .rfind(|frame| frame.start_address <= stack_index)
                .ok_or(RuntimeError::InvalidVmState(
                    "stack capture outside a call frame",
                ))?;
            owner.open_upvalues.push(OpenUpvalue {
                stack_index,
                closure_values,
                closure_index,
            });
        }
        Ok(())
    }

    fn return_value(
        &mut self,
        stack: &mut Vec<Val>,
        heap: &mut Heap,
    ) -> Result<ValOrCallFrame<'_>, RuntimeError> {
        // Validate the return before mutating the frame or stack.
        let call_frame = self
            .0
            .last()
            .ok_or(RuntimeError::InvalidVmState("return without a call frame"))?;
        let return_value = stack
            .get(call_frame.operand_stack_start..)
            .and_then(|operands| operands.last())
            .copied()
            .ok_or(RuntimeError::InvalidBytecode("return without a value"))?;
        if call_frame.return_address >= stack.len() || call_frame.start_address > stack.len() {
            return Err(RuntimeError::InvalidVmState(
                "call frame return address is outside the stack",
            ));
        }
        let return_address = call_frame.return_address;

        // 1. Pop call frame
        let call_frame = self.0.pop().ok_or(RuntimeError::StackUnderflow)?;

        // 2. Close upvalues
        close_upvalues(&call_frame.open_upvalues, stack, heap);

        // 3. Replace the entire call frame with its return value.
        stack.truncate(return_address);
        stack.push(return_value);

        let result = self
            .0
            .last_mut()
            .map(ValOrCallFrame::CallFrame)
            .unwrap_or(ValOrCallFrame::Val(return_value));
        Ok(result)
    }

    /// Pushes a placeholder frame for a native function that returned an error.
    ///
    /// Native functions have no module, bytecode, or locals, so the frame only
    /// records the function value and its argument slots for the backtrace.
    ///
    /// `args` is the range of stack slots holding the arguments, with the
    /// function value immediately below them at `args.start - 1`.
    fn push_synthetic(&mut self, module: HeapHandle<Module>, function: Val, args: Range<usize>) {
        self.0.push(CallFrame {
            module,
            instructions: Arc::default(),
            function,
            closure_values: None,
            open_upvalues: SmallVec::new(),
            instruction_idx: 0,
            return_address: args.start - 1,
            start_address: args.start,
            operand_stack_start: args.end,
        });
    }

    /// Validates the call and pushes a call frame for a bytecode function or
    /// closure, reserving stack space for its locals.
    ///
    /// `args_start` must be the stack index of the first argument, with the
    /// function value immediately below it at `args_start - 1`.
    fn push_bytecode_frame(
        &mut self,
        stack: &mut Vec<Val>,
        heap: &Heap,
        bytecode_handle: HeapHandle<BytecodeFunction>,
        closure_values: Option<HeapHandle<ClosureCaptures>>,
        args_start: usize,
        arg_count: usize,
    ) -> Result<&mut CallFrame, RuntimeError> {
        let bytecode_function =
            heap.bytecode_functions
                .get(bytecode_handle)
                .ok_or(RuntimeError::InvalidVmState(
                    "bytecode function was collected",
                ))?;
        bytecode_function.arity.validate(arg_count)?;
        let module = bytecode_function.module;
        let instructions = Arc::clone(&bytecode_function.bytecode);
        let local_count = bytecode_function.local_count as usize;
        let operand_stack_start = stack
            .len()
            .checked_add(local_count)
            .ok_or(RuntimeError::StackUnderflow)?;
        let call_frame = CallFrame {
            module,
            instructions,
            function: stack[args_start - 1],
            closure_values,
            open_upvalues: SmallVec::new(),
            instruction_idx: 0,
            return_address: args_start - 1,
            start_address: args_start,
            operand_stack_start,
        };
        stack.extend(std::iter::repeat_n(Val::Undefined, local_count));
        Ok(self.0.push_mut(call_frame))
    }
}

impl CallFrame {
    /// Reads the upvalue at `slot` of this frame's closure.
    ///
    /// Captures that still point into the VM stack are read from `stack`; once
    /// the frame that owns their slot has returned they are read from a box.
    fn upvalue(&self, slot: UpvalueSlot, heap: &Heap, stack: &[Val]) -> Result<Val, RuntimeError> {
        let closure_handle = self.closure_values.ok_or(RuntimeError::InvalidVmState(
            "Attempted to access closure values outside of closure",
        ))?;
        let capture = *heap
            .closures
            .get(closure_handle)
            .ok_or(RuntimeError::InvalidVmState("closure was collected"))?
            .values
            .get(slot.index())
            .ok_or(RuntimeError::InvalidVmState(
                "upvalue index is outside the closure",
            ))?;
        match capture {
            CaptureStorage::Stack(stack_index) => {
                stack
                    .get(stack_index)
                    .copied()
                    .ok_or(RuntimeError::InvalidVmState(
                        "captured stack slot is outside the stack",
                    ))
            }
            CaptureStorage::Boxed(boxed_handle) | CaptureStorage::ReadOnlyBoxed(boxed_handle) => {
                heap.boxed_values
                    .get(boxed_handle)
                    .map(BoxedValue::value)
                    .ok_or(RuntimeError::InvalidVmState("boxed capture was collected"))
            }
        }
    }

    fn set_upvalue(
        &self,
        slot: UpvalueSlot,
        val: Val,
        heap: &mut Heap,
        stack: &mut [Val],
    ) -> Result<(), RuntimeError> {
        let closure_handle = self.closure_values.ok_or(RuntimeError::InvalidVmState(
            "Attempted to access closure values outside of closure",
        ))?;
        let capture = *heap
            .closures
            .get(closure_handle)
            .ok_or(RuntimeError::InvalidVmState("closure was collected"))?
            .values
            .get(slot.index())
            .ok_or(RuntimeError::InvalidVmState(
                "upvalue index is outside the closure",
            ))?;
        match capture {
            CaptureStorage::Stack(stack_index) => {
                let dst = stack
                    .get_mut(stack_index)
                    .ok_or(RuntimeError::InvalidVmState(
                        "captured stack slot is outside the stack",
                    ))?;
                *dst = val;
            }
            CaptureStorage::Boxed(boxed_handle) => {
                let dst = heap
                    .boxed_values
                    .get_mut(boxed_handle)
                    .ok_or(RuntimeError::InvalidVmState("boxed capture was collected"))?;
                *dst.value_mut() = val;
            }
            CaptureStorage::ReadOnlyBoxed(_) => {
                std::hint::cold_path();
                return Err(RuntimeError::InvalidBytecode(
                    "assignment to immutable capture",
                ));
            }
        }
        Ok(())
    }

    /// Builds this frame's captures for `bytecode_handle` and inserts the
    /// resulting closure into the heap.
    ///
    /// Returns the new closure together with the captures that point into the VM
    /// stack. The caller must open those captures with the call frames that own
    /// their stack slots, which [`CallFrames::make_closure`] does for you.
    fn build_captures(
        &self,
        heap: &mut Heap,
        bytecode_handle: HeapHandle<BytecodeFunction>,
    ) -> Result<(HeapHandle<ClosureCaptures>, StackCaptures), RuntimeError> {
        let captures = heap
            .bytecode_functions
            .get(bytecode_handle)
            .ok_or(RuntimeError::InvalidVmState(
                "cannot capture from a collected bytecode function",
            ))?
            .captures
            .clone();
        let mut values = Vec::with_capacity(captures.len());
        let mut stack_captures = StackCaptures::new();
        for (closure_index, capture) in captures.iter().enumerate() {
            let value = match capture {
                CaptureSource::Local(index) => {
                    let stack_index = self.start_address + index;
                    stack_captures.push((closure_index, stack_index));
                    CaptureStorage::Stack(stack_index)
                }
                CaptureSource::Upvalue(index) => {
                    let enclosing_closure = self.closure_values.ok_or(
                        RuntimeError::InvalidVmState("upvalue capture outside a closure"),
                    )?;
                    let value = *heap
                        .closures
                        .get(enclosing_closure)
                        .ok_or(RuntimeError::InvalidVmState(
                            "enclosing closure was collected",
                        ))?
                        .values
                        .get(*index)
                        .ok_or(RuntimeError::InvalidVmState(
                            "upvalue capture index is outside the enclosing closure",
                        ))?;
                    if let CaptureStorage::Stack(stack_index) = value {
                        stack_captures.push((closure_index, stack_index));
                    }
                    value
                }
                CaptureSource::CurrentFunction => {
                    let boxed = heap
                        .boxed_values
                        .insert(BoxedValue::from_value(self.function));
                    CaptureStorage::ReadOnlyBoxed(boxed)
                }
            };
            values.push(value);
        }
        let closure_handle = heap.closures.insert(ClosureCaptures::from_captures(
            bytecode_handle,
            values.into_boxed_slice(),
        ));
        Ok((closure_handle, stack_captures))
    }
}

/// Boxes the stack slots captured by `open_upvalues` and rewires the closures
/// that read them, so that they survive the call frame that owns the slots.
///
/// Captures of the same slot share a single box. Captures whose slot is no
/// longer on the stack, or whose closure is already gone, are skipped.
fn close_upvalues(open_upvalues: &[OpenUpvalue], stack: &[Val], heap: &mut Heap) {
    let mut boxed_values = SmallVec::<[(usize, HeapHandle<BoxedValue>); 2]>::new();
    for open_upvalue in open_upvalues {
        let Some(stack_value) = stack.get(open_upvalue.stack_index).copied() else {
            std::hint::cold_path();
            continue;
        };
        let boxed_handle = match boxed_values
            .iter()
            .find(|(stack_index, _)| *stack_index == open_upvalue.stack_index)
        {
            Some((_, handle)) => *handle,
            None => {
                let handle = heap
                    .boxed_values
                    .insert(BoxedValue::from_value(stack_value));
                boxed_values.push((open_upvalue.stack_index, handle));
                handle
            }
        };
        if let Some(closure) = heap.closures.get_mut(open_upvalue.closure_values)
            && let Some(capture) = closure.values.get_mut(open_upvalue.closure_index)
        {
            *capture = CaptureStorage::Boxed(boxed_handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::error::{CompilerError, CompilerErrorKind, VmErrorConfig, VmErrorKind};
    use crate::function::{Arity, BytecodeFunction};
    use crate::native_function::NativeClosure;
    use crate::pair::Pair;

    use super::*;

    struct TempSource {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempSource {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            loop {
                let dir = std::env::temp_dir().join(format!(
                    "takolisp-load-{}-{}",
                    std::process::id(),
                    NEXT_ID.fetch_add(1, Ordering::Relaxed)
                ));
                match std::fs::create_dir(&dir) {
                    Ok(()) => {
                        return Self {
                            path: dir.join("source.tako"),
                            dir,
                        };
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create temporary source directory: {error}"),
                }
            }
        }

        fn write(&self, source: &str) {
            std::fs::write(&self.path, source).unwrap();
        }
    }

    impl Drop for TempSource {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_dir(&self.dir);
        }
    }

    #[test]
    fn missing_file_reports_path_and_can_be_loaded_after_it_appears() {
        let source = TempSource::new();
        let mut vm = Vm::new();

        let error = vm.ensure_loaded(&source.path).unwrap_err();
        assert!(matches!(error.kind, VmErrorKind::FileRead { .. }));
        let message = error.display(&vm, VmErrorConfig::default()).to_string();
        assert!(message.contains("cannot read"));
        assert!(message.contains(&source.path.display().to_string()));

        source.write("(define answer 42)");
        let module = vm.ensure_loaded(&source.path).unwrap();
        assert_eq!(
            vm.heap
                .modules
                .get(module)
                .unwrap()
                .get(Symbol::must_inline("answer")),
            Val::Int(42)
        );

        source.write("(define answer 99)");
        assert_eq!(vm.ensure_loaded(&source.path), Ok(module));
        assert_eq!(
            vm.heap
                .modules
                .get(module)
                .unwrap()
                .get(Symbol::must_inline("answer")),
            Val::Int(42)
        );
    }

    #[test]
    fn invalid_source_can_be_repaired_and_loaded() {
        for (invalid, expected_error) in [
            ("(define", "parse"),
            ("missing", "compile"),
            ("(define answer 1) ((lambda (x) x))", "runtime"),
        ] {
            let source = TempSource::new();
            let mut vm = Vm::new();
            source.write(invalid);

            let error = vm.ensure_loaded(&source.path).unwrap_err();
            assert!(
                match expected_error {
                    "parse" => matches!(error.kind, VmErrorKind::AstErrors { .. }),
                    "compile" => matches!(error.kind, VmErrorKind::Compiler { .. }),
                    "runtime" => matches!(error.kind, VmErrorKind::Runtime(_)),
                    _ => unreachable!(),
                },
                "unexpected error for {invalid}: {error:?}"
            );
            if expected_error == "runtime" {
                let message = error.display(&vm, VmErrorConfig::default()).to_string();
                assert!(message.contains("backtrace:"));
            }

            source.write("(define fixed 42)");
            let module = vm.ensure_loaded(&source.path).unwrap();
            assert_eq!(
                vm.heap
                    .modules
                    .get(module)
                    .unwrap()
                    .get(Symbol::must_inline("fixed")),
                Val::Int(42),
                "repaired source did not evaluate after {expected_error} error"
            );
            if expected_error == "runtime" {
                assert_eq!(
                    vm.heap
                        .modules
                        .get(module)
                        .unwrap()
                        .get(Symbol::must_inline("answer")),
                    Val::Undefined
                );
            }
        }
    }

    #[test]
    fn lookup_slot_uses_each_functions_module_and_restores_callers_module() {
        let mut vm = Vm::new();
        let caller_module = vm
            .heap
            .modules
            .insert(ModuleSource::Internal("test"))
            .unwrap();
        let callee_module = vm
            .heap
            .modules
            .insert(ModuleSource::Internal("callee"))
            .unwrap();
        let symbol = Symbol::must_inline("value");
        for (module, value) in [(caller_module, 11), (callee_module, 22)] {
            vm.heap
                .modules
                .get_mut(module)
                .unwrap()
                .set(symbol, Val::Int(value));
        }
        let slot = vm
            .heap
            .modules
            .get(callee_module)
            .unwrap()
            .slot(symbol)
            .unwrap();
        let callee = vm.heap.bytecode_functions.insert(BytecodeFunction {
            bytecode: [Instruction::LookupSlot(slot), Instruction::Return].into(),
            ..BytecodeFunction::new(callee_module)
        });
        let captures = vm
            .heap
            .closures
            .insert(ClosureCaptures::from_captures(callee, Default::default()));
        for callee in [Val::BytecodeFunction(callee), Val::Closure(captures)] {
            let caller = vm.heap.bytecode_functions.insert(BytecodeFunction {
                bytecode: [
                    Instruction::PushConst(callee),
                    Instruction::Eval(0),
                    Instruction::Return,
                ]
                .into(),
                ..BytecodeFunction::new(caller_module)
            });
            assert_eq!(vm.eval(caller.into(), []), Ok(Val::Int(22)));
            vm.heap.bytecode_functions.get_mut(caller).unwrap().bytecode = [
                Instruction::PushConst(callee),
                Instruction::Eval(0),
                Instruction::PopN(1),
                Instruction::LookupSlot(slot),
                Instruction::Return,
            ]
            .into();
            assert_eq!(vm.eval(caller.into(), []), Ok(Val::Int(11)));
        }
    }

    #[test]
    fn undefined_slot_reports_symbol_from_functions_module() {
        let mut vm = Vm::new();
        let module = vm
            .heap
            .modules
            .insert(ModuleSource::Internal("test"))
            .unwrap();
        let symbol = Symbol::must_inline("missing");
        let slot = vm.heap.modules.get_mut(module).unwrap().define(symbol);
        let function = vm.heap.bytecode_functions.insert(BytecodeFunction {
            bytecode: [Instruction::LookupSlot(slot), Instruction::Return].into(),
            ..BytecodeFunction::new(module)
        });
        assert_eq!(
            vm.eval(function.into(), []),
            Err(RuntimeError::ValueNotFound(Some(symbol)))
        );
    }

    #[test]
    fn eval_bytecode_function_accepts_exact_arity() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let func = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction {
                module,
                name: None,
                arity: Arity::Exact(2),
                local_count: 0,
                captures: [].into(),
                bytecode: [Instruction::PushConst(Val::Int(42)), Instruction::Return].into(),
            })
            .into();

        assert_eq!(vm.eval(func, [Val::Int(1), Val::Int(2)]), Ok(Val::Int(42)));
    }

    #[test]
    fn return_requires_an_operand_owned_by_the_current_frame() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let func = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction {
                module,
                arity: Arity::Exact(0),
                bytecode: [Instruction::Return].into(),
                ..BytecodeFunction::new(module)
            })
            .into();

        assert_eq!(
            vm.eval(func, []),
            Err(RuntimeError::InvalidBytecode("return without a value"))
        );
    }

    #[test]
    fn stack_operations_cannot_consume_the_call_frame() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let func = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction {
                module,
                arity: Arity::Exact(0),
                bytecode: [Instruction::PopN(1), Instruction::Return].into(),
                ..BytecodeFunction::new(module)
            })
            .into();

        assert_eq!(vm.eval(func, []), Err(RuntimeError::StackUnderflow));
    }

    #[test]
    fn get_upvalue_reads_stack_and_boxed_captures() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let stack_function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(1),
            bytecode: [
                Instruction::GetUpvalue(UpvalueSlot::new(0)),
                Instruction::Return,
            ]
            .into(),
            ..BytecodeFunction::new(module)
        });
        let stack_closure = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(
                stack_function,
                [CaptureStorage::Stack(1)].into(),
            ));
        assert_eq!(
            vm.eval(Val::Closure(stack_closure), [Val::Int(42)]),
            Ok(Val::Int(42))
        );

        let boxed = vm
            .heap_mut()
            .boxed_values
            .insert(BoxedValue::from_value(Val::Int(7)));
        let boxed_function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            bytecode: [
                Instruction::GetUpvalue(UpvalueSlot::new(0)),
                Instruction::Return,
            ]
            .into(),
            ..BytecodeFunction::new(module)
        });
        let boxed_closure = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(
                boxed_function,
                [CaptureStorage::ReadOnlyBoxed(boxed)].into(),
            ));
        assert_eq!(vm.eval(Val::Closure(boxed_closure), []), Ok(Val::Int(7)));
    }

    #[test]
    fn calling_a_collected_function_or_closure_is_an_error() {
        let mut vm = Vm::new();
        let module = vm
            .heap_mut()
            .modules
            .insert(ModuleSource::Internal("test"))
            .unwrap();
        let function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            bytecode: [Instruction::PushConst(Val::Int(1)), Instruction::Return].into(),
            ..BytecodeFunction::new(module)
        });
        let bytecode_val: Val = function.into();
        let closure = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(function, [].into()));
        let closure_val = Val::Closure(closure);
        assert_eq!(vm.eval(bytecode_val, []), Ok(Val::Int(1)));

        vm.collect_garbage();

        for stale in [bytecode_val, closure_val] {
            assert!(matches!(
                vm.eval(stale, []),
                Err(RuntimeError::InvalidVmState(_))
            ));
        }
    }

    #[test]
    fn closure_with_a_collected_function_is_an_error() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            bytecode: [Instruction::PushConst(Val::Int(1)), Instruction::Return].into(),
            ..BytecodeFunction::new(module)
        });
        let closure = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(function, [].into()));
        vm.heap_mut().bytecode_functions.remove(function);

        assert!(matches!(
            vm.eval(Val::Closure(closure), []),
            Err(RuntimeError::InvalidVmState(_))
        ));
    }

    #[test]
    fn make_closure_of_a_collected_function_is_an_error() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let dead = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction::new(module));
        vm.heap_mut().bytecode_functions.remove(dead);
        let outer = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            bytecode: [
                Instruction::MakeClosure(dead),
                Instruction::Eval(0),
                Instruction::Return,
            ]
            .into(),
            ..BytecodeFunction::new(module)
        });

        assert!(matches!(
            vm.eval(outer.into(), []),
            Err(RuntimeError::InvalidVmState(_))
        ));
    }

    #[test]
    fn upvalue_capture_without_an_enclosing_closure_is_an_error() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let inner = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            captures: [CaptureSource::Upvalue(0)].into(),
            bytecode: [
                Instruction::GetUpvalue(UpvalueSlot::new(0)),
                Instruction::Return,
            ]
            .into(),
            ..BytecodeFunction::new(module)
        });
        let outer = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            bytecode: [
                Instruction::MakeClosure(inner),
                Instruction::Eval(0),
                Instruction::Return,
            ]
            .into(),
            ..BytecodeFunction::new(module)
        });

        assert!(matches!(
            vm.eval(outer.into(), []),
            Err(RuntimeError::InvalidVmState(_))
        ));
    }

    #[test]
    fn get_upvalue_rejects_invalid_captures() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let get_upvalue = |vm: &mut Vm, slot| {
            vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
                module,
                arity: Arity::Exact(0),
                bytecode: [Instruction::GetUpvalue(slot), Instruction::Return].into(),
                ..BytecodeFunction::new(module)
            })
        };

        let out_of_range = get_upvalue(&mut vm, UpvalueSlot::new(5));
        let out_of_range = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(
                out_of_range,
                [CaptureStorage::Stack(1)].into(),
            ));
        assert!(matches!(
            vm.eval(Val::Closure(out_of_range), []),
            Err(RuntimeError::InvalidVmState(_))
        ));

        let stale_stack = get_upvalue(&mut vm, UpvalueSlot::new(0));
        let stale_stack = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(
                stale_stack,
                [CaptureStorage::Stack(9_999)].into(),
            ));
        assert!(matches!(
            vm.eval(Val::Closure(stale_stack), []),
            Err(RuntimeError::InvalidVmState(_))
        ));

        let collected = vm
            .heap_mut()
            .boxed_values
            .insert(BoxedValue::from_value(Val::Int(1)));
        vm.heap_mut().boxed_values.remove(collected);
        let stale_box = get_upvalue(&mut vm, UpvalueSlot::new(0));
        let stale_box = vm
            .heap_mut()
            .closures
            .insert(ClosureCaptures::from_captures(
                stale_box,
                [CaptureStorage::Boxed(collected)].into(),
            ));
        assert!(matches!(
            vm.eval(Val::Closure(stale_box), []),
            Err(RuntimeError::InvalidVmState(_))
        ));
    }

    #[test]
    fn local_instructions_reject_slots_outside_the_stack() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        for instruction in [Instruction::GetLocal(500), Instruction::SetLocal(500)] {
            let function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
                module,
                arity: Arity::Exact(0),
                bytecode: [instruction, Instruction::Return].into(),
                ..BytecodeFunction::new(module)
            });
            assert!(matches!(
                vm.eval(function.into(), []),
                Err(RuntimeError::InvalidVmState(_))
            ));
        }
    }

    #[test]
    fn define_rejects_slots_outside_the_module() {
        let mut vm = Vm::new();
        // An empty module has no slots at all, so every `Define` is out of range.
        let module = vm
            .heap_mut()
            .modules
            .insert(ModuleSource::Internal("test"))
            .unwrap();
        let function = vm.heap_mut().bytecode_functions.insert(BytecodeFunction {
            module,
            arity: Arity::Exact(0),
            bytecode: [
                Instruction::PushConst(Val::Int(1)),
                Instruction::Define(crate::module::ModuleSlot::new(0)),
                Instruction::Return,
            ]
            .into(),
            ..BytecodeFunction::new(module)
        });

        assert!(matches!(
            vm.eval(function.into(), []),
            Err(RuntimeError::InvalidVmState(_))
        ));
    }

    #[test]
    fn closures_that_escape_a_failed_call_keep_their_captured_values() {
        let mut vm = Vm::new();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        // `g` escapes into the module while `f`'s frame is still open, then the
        // frame is abandoned by a runtime error instead of returning.
        assert!(
            vm.eval_str(
                module,
                "(define (f a b c) (define g (lambda () c)) ((lambda (x y) x) 1)) (f 1 2 333)"
            )
            .is_err()
        );

        assert_eq!(vm.eval_str(module, "(g)"), Ok(Val::Int(333)));
    }

    #[test]
    fn eval_bytecode_function_rejects_wrong_arity() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let func = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction {
                module,
                name: None,
                arity: Arity::Exact(2),
                local_count: 0,
                captures: [].into(),
                bytecode: [Instruction::PushConst(Val::Int(42)), Instruction::Return].into(),
            })
            .into();

        assert_eq!(
            vm.eval(func, [Val::Int(1)]),
            Err(RuntimeError::ArityError {
                expected: Arity::Exact(2),
                found: 1,
            })
        );
        assert_eq!(
            vm.eval(func, [Val::Int(1), Val::Int(2), Val::Int(3)]),
            Err(RuntimeError::ArityError {
                expected: Arity::Exact(2),
                found: 3,
            })
        );
    }

    #[test]
    fn nested_bytecode_function_checks_arity() {
        let mut vm = Vm::new();
        let module = vm.global_module();
        let inner = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction {
                module,
                name: None,
                arity: Arity::Exact(1),
                local_count: 0,
                captures: [].into(),
                bytecode: vec![Instruction::PushConst(Val::Int(42)), Instruction::Return].into(),
            })
            .into();
        let outer = vm
            .heap_mut()
            .bytecode_functions
            .insert(BytecodeFunction {
                module,
                name: None,
                arity: Arity::Exact(0),
                local_count: 0,
                captures: [].into(),
                bytecode: vec![
                    Instruction::PushConst(inner),
                    Instruction::Eval(0),
                    Instruction::Return,
                ]
                .into(),
            })
            .into();

        assert_eq!(
            vm.eval(outer, std::iter::empty()),
            Err(RuntimeError::ArityError {
                expected: Arity::Exact(1),
                found: 0,
            })
        );
    }

    #[test]
    fn native_closure_context_receives_args_from_direct_and_bytecode_calls() {
        let mut vm = Vm::new();
        let mut counter = 0;
        let func = vm.heap.native_closures.insert(NativeClosure::from_fn(
            "increment-counter",
            move |context| {
                for arg in context.args() {
                    let Val::Int(x) = arg else {
                        panic!();
                    };
                    counter += *x;
                }
                Ok(counter.into())
            },
        ));

        assert_eq!(vm.eval(func.into(), [Val::Int(1)]), Ok(Val::Int(1)));
        assert_eq!(
            vm.eval(func.into(), [Val::Int(1), Val::Int(2)]),
            Ok(Val::Int(4))
        );
        assert_eq!(vm.eval(func.into(), [Val::Int(1)]), Ok(Val::Int(5)));
    }

    #[test]
    fn eval_returns_last_expression() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "1 2 (+ 1 2 3 4 (+ 5 6))"),
            Ok(Val::Int(21))
        );
    }

    #[test]
    fn eval_define_stores_global_and_returns_undefined() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(define x 42)"),
            Ok(Val::Undefined)
        );
        assert_eq!(vm.eval_str(vm.global_module(), "x"), Ok(Val::Int(42)));
    }

    #[test]
    fn set_returns_undefined_for_module_and_captured_bindings() {
        for source in [
            "(define x 1) (set! x 42)",
            "(((lambda (x) (lambda () (set! x 42))) 1))",
        ] {
            let mut vm = Vm::new();
            assert_eq!(
                vm.eval_str(vm.global_module(), source),
                Ok(Val::Undefined),
                "{source}"
            );
        }
    }

    #[test]
    fn set_returns_undefined_for_arguments() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "((lambda (x) (set! x 42)) 1)"),
            Ok(Val::Undefined)
        );
    }

    #[test]
    fn set_returns_undefined_for_let_bindings() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(let ((x 1)) (set! x 42))"),
            Ok(Val::Undefined)
        );
    }

    #[test]
    fn set_updates_existing_bindings() {
        for source in [
            "(define x 1) (set! x (+ x 41)) x",
            "((lambda (x) (set! x (+ x 41)) x) 1)",
            "((lambda (arg) (let ((x 1)) (set! x (+ x arg)) x)) 41)",
            "(begin (let ((unused 0)) unused) (let ((x 1)) (set! x 42) x))",
        ] {
            let mut vm = Vm::new();
            assert_eq!(
                vm.eval_str(vm.global_module(), source),
                Ok(Val::Int(42)),
                "{source}"
            );
        }
    }

    #[test]
    fn set_changes_only_the_nearest_binding() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define x 100)
                 ((lambda (x)
                    (+ (let ((x 1))
                         (+ (let ((x 2)) (set! x 42) x) x))
                       x)) 10)"
            ),
            Ok(Val::Int(53))
        );
        assert_eq!(vm.eval_str(vm.global_module(), "x"), Ok(Val::Int(100)));
    }

    #[test]
    fn set_evaluates_its_value_once_before_changing_the_binding() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define x 10) (define calls 0)
                 (set! x (begin (set! calls (+ calls 1)) (+ x 1)))
                 (+ x calls)"
            ),
            Ok(Val::Int(12))
        );
    }

    #[test]
    fn set_preserves_the_old_value_when_evaluation_fails() {
        let mut vm = Vm::new();
        vm.eval_str(vm.global_module(), "(define x 42)").unwrap();
        let error = vm
            .eval_str(vm.global_module(), "(set! x (+ #true))")
            .unwrap_err();
        assert!(matches!(
            error.kind,
            VmErrorKind::Runtime(RuntimeError::TypeError { .. })
        ));
        assert_eq!(vm.eval_str(vm.global_module(), "x"), Ok(Val::Int(42)));
    }

    #[test]
    fn set_in_a_closure_changes_the_active_enclosing_binding() {
        for source in [
            "((lambda (x) ((lambda () (set! x 42))) x) 1)",
            "(let ((x 1)) ((lambda () (set! x 42))) x)",
            "(let ((x 1) (read (lambda () x))) (set! x 42) (read))",
        ] {
            let mut vm = Vm::new();
            assert_eq!(
                vm.eval_str(vm.global_module(), source),
                Ok(Val::Int(42)),
                "{source}"
            );
        }
    }

    #[test]
    fn set_is_shared_by_sibling_closures_after_the_enclosing_call_returns() {
        let mut vm = Vm::new();
        vm.eval_str(
            vm.global_module(),
            "((lambda (x)
                (define read (lambda () x))
                (define write (lambda (value) (set! x value)))) 1)",
        )
        .unwrap();
        assert_eq!(vm.eval_str(vm.global_module(), "(read)"), Ok(Val::Int(1)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(write 42)"),
            Ok(Val::Undefined)
        );
        assert_eq!(vm.eval_str(vm.global_module(), "(read)"), Ok(Val::Int(42)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(write 7) (read)"),
            Ok(Val::Int(7))
        );
    }

    #[test]
    fn set_through_nested_closures_retains_independent_factory_state() {
        let mut vm = Vm::new();
        vm.eval_str(
            vm.global_module(),
            "(define (factory x)
                (lambda () (lambda () (set! x (+ x 1)) x)))
             (define first ((factory 0)))
             (define second ((factory 10)))",
        )
        .unwrap();
        for (source, expected) in [("(first)", 1), ("(second)", 11), ("(first)", 2)] {
            assert_eq!(
                vm.eval_str(vm.global_module(), source),
                Ok(Val::Int(expected))
            );
        }
    }

    #[test]
    fn set_keeps_new_captured_values_alive_through_garbage_collection() {
        let mut vm = Vm::new();
        vm.eval_str(
            vm.global_module(),
            r#"((lambda (x)
                (define read (lambda () x))
                (define write (lambda (value) (set! x value)))) 0)
             (write "saved")"#,
        )
        .unwrap();
        vm.eval_str(vm.global_module(), "0").unwrap();
        vm.collect_garbage();
        let value = vm.eval_str(vm.global_module(), "(read)").unwrap();
        assert_eq!(format!("{:?}", value.display(&vm)), "\"saved\"");
    }

    #[test]
    fn set_uses_the_functions_module_when_called_from_another_module() {
        let mut vm = Vm::new();
        let owner = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        let caller = vm
            .create_root_module(ModuleSource::Internal("caller"))
            .unwrap();
        vm.eval_str(owner, "(define x 1) (define (write) (set! x 42))")
            .unwrap();
        let write = vm.eval_str(owner, "write").unwrap();
        vm.heap_mut()
            .modules
            .get_mut(caller)
            .unwrap()
            .set(Symbol::must_inline("write"), write);
        assert_eq!(
            vm.eval_str(caller, "(define x 100) (write) x"),
            Ok(Val::Int(100))
        );
        assert_eq!(vm.eval_str(owner, "x"), Ok(Val::Int(42)));
    }

    #[test]
    fn set_rejects_unbound_names_and_immutable_bindings() {
        for (source, expected) in [
            (
                "(set! missing 42)",
                CompilerErrorKind::UnboundSymbol(Symbol::must_inline("missing")),
            ),
            (
                "(set! + 42)",
                CompilerErrorKind::ImmutableSymbol(Symbol::must_inline("+")),
            ),
            (
                "(define (recur) (set! recur 42))",
                CompilerErrorKind::ImmutableSymbol(Symbol::must_inline("recur")),
            ),
        ] {
            let mut vm = Vm::new();
            let module = vm
                .create_root_module(ModuleSource::Internal("test"))
                .unwrap();
            let error = vm.eval_str(module, source).unwrap_err();
            assert!(
                matches!(error.kind, VmErrorKind::Compiler { error, .. } if error.kind == expected),
                "{source}"
            );
            if matches!(expected, CompilerErrorKind::UnboundSymbol(_)) {
                assert!(vm.eval_str(module, "missing").is_err());
            }
            assert_eq!(vm.eval_str(module, "(+ 1 2)"), Ok(Val::Int(3)));
        }
    }

    #[test]
    fn set_cannot_change_an_enclosing_functions_name() {
        for source in [
            "(define (recur) (lambda () (set! recur 42)))",
            "(define (recur) (lambda () (lambda () (set! recur 42))))",
            "(define (recur) (lambda () recur (set! recur 42)))",
            "(define (recur) (lambda () recur (lambda () (set! recur 42))))",
        ] {
            let mut vm = Vm::new();
            let error = vm.eval_str(vm.global_module(), source).unwrap_err();
            assert!(matches!(
                error.kind,
                VmErrorKind::Compiler {
                    error: CompilerError {
                        kind: CompilerErrorKind::ImmutableSymbol(symbol),
                        ..
                    },
                    ..
                } if symbol == Symbol::must_inline("recur")
            ));
        }
    }

    #[test]
    fn set_allows_mutable_bindings_that_shadow_function_names() {
        for source in [
            "(define (recur recur) (set! recur 42) recur) (recur 1)",
            "(define (recur) (let ((recur 1)) (set! recur 42) recur)) (recur)",
            "(define (recur recur) (lambda () (set! recur 42) recur)) ((recur 1))",
            "(define (recur) (let ((recur 1)) (lambda () (set! recur 42) recur))) ((recur))",
            "(define (recur) (lambda (recur) (lambda () (set! recur 42) recur))) (((recur) 1))",
        ] {
            let mut vm = Vm::new();
            assert_eq!(
                vm.eval_str(vm.global_module(), source),
                Ok(Val::Int(42)),
                "{source}"
            );
        }
    }

    #[test]
    fn collect_garbage_reclaims_unreachable_values_and_preserves_globals() {
        let mut vm = Vm::new();
        let dead = vm.heap_mut().strings.insert("dead".to_string());
        let live = vm.heap_mut().strings.insert("live".to_string());
        let global_module = vm.global_module();
        vm.heap_mut()
            .modules
            .get_mut(global_module)
            .unwrap()
            .set(Symbol::must_inline("live"), live.into());
        let unused_module = vm
            .heap_mut()
            .modules
            .insert(ModuleSource::Internal("test"))
            .unwrap();

        for _ in 0..2 {
            vm.collect_garbage();

            assert!(vm.heap().modules.get(unused_module).is_some());
            assert!(vm.heap().modules.get(global_module).is_some());
            assert!(vm.heap().strings.get(dead).is_none());
            assert_eq!(vm.heap().strings.get(live), Some(&"live".to_string()));
            assert_eq!(vm.eval_str(vm.global_module(), "live"), Ok(live.into()));
        }
    }

    #[test]
    fn all_modules_and_their_transitive_values_survive_collection() {
        let mut vm = Vm::new();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        let unused = vm
            .heap_mut()
            .modules
            .insert(ModuleSource::Internal("unused"))
            .unwrap();
        let string = vm.heap_mut().strings.insert("rooted");
        let list = vm.heap_mut().pairs.insert(Pair {
            car: string.into(),
            cdr: Val::Nil,
        });
        vm.heap_mut()
            .modules
            .get_mut(module)
            .unwrap()
            .set(Symbol::must_inline("saved"), Val::Pair(list));
        for _ in 0..3 {
            vm.collect_garbage();
            assert!(vm.heap().modules.get(unused).is_some());
            assert_eq!(vm.heap().strings.get(string).unwrap(), "rooted");
            assert_eq!(vm.heap().pairs.get(list).unwrap().car, string.into());
            assert_eq!(vm.heap().pairs.get(list).unwrap().cdr, Val::Nil);
        }
    }

    #[test]
    fn failed_native_calls_record_a_frame_with_the_null_module() {
        let mut vm = Vm::new();
        let null_module = vm.null_module;

        // All modules survive collection, so the placeholder always resolves.
        for _ in 0..2 {
            vm.collect_garbage();
            assert!(vm.heap().modules.get(null_module).is_some());
        }

        let error = vm
            .eval_str(vm.global_module(), "(+ #true)")
            .expect_err("adding a boolean is a type error");
        assert!(matches!(
            error.kind,
            VmErrorKind::Runtime(RuntimeError::TypeError { .. })
        ));
        assert_eq!(
            vm.call_frames
                .current()
                .expect("the failed native call leaves a frame")
                .module,
            null_module
        );
    }

    #[test]
    fn custom_module_preserves_new_globals_and_recursive_functions() {
        let mut vm = Vm::new();
        let module = vm
            .create_root_module(ModuleSource::Internal("test"))
            .unwrap();
        vm.collect_garbage();
        vm.eval_str(
            module,
            "(define saved ((lambda (x) (lambda () x)) \"live\"))",
        )
        .unwrap();
        vm.eval_str(
            module,
            "(define (count n) (if (< n 1) (saved) (count (- n 1))))",
        )
        .unwrap();
        for _ in 0..3 {
            // Clear the evaluation stack so only the explicit module root keeps it alive.
            vm.eval_str(vm.global_module(), "0").unwrap();
            vm.collect_garbage();
            let value = vm.eval_str(module, "(count 3)").unwrap();
            assert_eq!(format!("{:?}", value.display(&vm)), "\"live\"");
        }
        assert!(vm.eval_str(vm.global_module(), "saved").is_err());
        assert!(vm.eval_str(vm.global_module(), "count").is_err());
    }

    #[test]
    fn eval_begin_returns_last_expression() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(begin (+ 1 2) (+ 3 4))"),
            Ok(Val::Int(7))
        );
    }

    #[test]
    fn eval_lambda_expression() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "((lambda () 42))"),
            Ok(Val::Int(42))
        );
    }

    #[test]
    fn eval_lambda_uses_arguments_as_locals() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "((lambda (x y) (+ x y)) 2 3)"),
            Ok(Val::Int(5))
        );
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define (adder x y) (+ x y)) (adder 4 5)"
            ),
            Ok(Val::Int(9))
        );
    }

    #[test]
    fn eval_closure_closes_an_enclosing_argument() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(((lambda (x) (lambda () x)) 42))"),
            Ok(Val::Int(42))
        );
    }

    #[test]
    fn eval_closure_closes_multiple_arguments_and_locals() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(((lambda (x y) (let ((z 3)) (lambda () (+ x y z)))) 10 20))"
            ),
            Ok(Val::Int(33))
        );
    }

    #[test]
    fn eval_nested_closure_captures_through_an_intermediate_closure() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "((((lambda (x) (lambda (y) (lambda () (+ x y)))) 10) 20))"
            ),
            Ok(Val::Int(30))
        );
    }

    #[test]
    fn eval_closure_capture_respects_shadowing() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(((lambda (x) (let ((x 2)) (lambda () x))) 1))"
            ),
            Ok(Val::Int(2))
        );
    }

    #[test]
    fn eval_closure_can_be_passed_to_another_function() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "((lambda (f) (f)) ((lambda (x) (lambda () x)) 42))"
            ),
            Ok(Val::Int(42))
        );
    }

    #[test]
    fn eval_closure_survives_unrelated_evaluations_and_repeated_calls() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define saved ((lambda (x) (lambda () x)) 42))"
            ),
            Ok(Val::Undefined)
        );
        assert_eq!(vm.eval_str(vm.global_module(), "(+ 1 2)"), Ok(Val::Int(3)));
        assert_eq!(
            vm.eval_str(vm.global_module(), "(+ (saved) (saved))"),
            Ok(Val::Int(84))
        );
    }

    #[test]
    fn eval_recursive_closure_captures_its_named_enclosing_function() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(),
                "(define (countdown n) (lambda () (if (< n 1) 0 ((countdown (- n 1)))))) ((countdown 3))"
            ),
            Ok(Val::Int(0))
        );
    }

    #[test]
    fn eval_closures_from_separate_factory_calls_keep_independent_captures() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(),
                "(define (factory x) (lambda () x)) (define first (factory 1)) (define second (factory 2)) (+ (first) (second))"
            ),
            Ok(Val::Int(3))
        );
    }

    #[test]
    fn eval_deeply_nested_closure_keeps_all_transitive_captures() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(((((lambda (a) (lambda (b) (lambda (c) (lambda (d) (+ a b c d))))) 1) 2) 3) 4)"
            ),
            Ok(Val::Int(10))
        );
    }

    #[test]
    fn eval_closure_created_by_an_early_return_is_still_callable() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define (factory) (return (lambda () 42)) 0) ((factory))"
            ),
            Ok(Val::Int(42))
        );
    }

    #[test]
    fn eval_closure_arity_error_does_not_poison_the_vm() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define saved ((lambda (x) (lambda () x)) 42))"
            ),
            Ok(Val::Undefined)
        );
        assert!(
            vm.eval_str(vm.global_module(), "((lambda (f) (f 1)) saved)")
                .is_err()
        );
        assert_eq!(vm.eval_str(vm.global_module(), "(saved)"), Ok(Val::Int(42)));
    }

    #[test]
    fn eval_closure_captures_locals_but_resolves_globals_at_call_time() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(define gval 7) (define saved ((lambda (x) (lambda () (+ x gval))) 5))"
            ),
            Ok(Val::Undefined)
        );
        assert_eq!(vm.set_global(Symbol::must_inline("gval"), Val::Int(10)), ());
        assert_eq!(vm.eval_str(vm.global_module(), "(saved)"), Ok(Val::Int(15)));
    }

    #[test]
    fn eval_let_bindings_sequentially() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(let ((x 1) (y (+ x 2))) (+ x y))"),
            Ok(Val::Int(4))
        );
    }

    #[test]
    fn eval_sibling_lets_use_distinct_local_slots() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                "(begin (let ((a 1)) a) (let ((b 2)) b))"
            ),
            Ok(Val::Int(2))
        );
    }

    #[test]
    fn eval_sibling_let_escaped_closure_captures_correct_slot() {
        let mut vm = Vm::new();
        // The closure must capture `b`'s real slot, not the position of `b` in
        // the truncatable local-name list.
        vm.eval_str(
            vm.global_module(),
            "(begin (let ((a 1)) a) (let ((b 2)) (define fb (lambda () b))))",
        )
        .unwrap();
        assert_eq!(vm.eval_str(vm.global_module(), "(fb)"), Ok(Val::Int(2)));
    }

    #[test]
    fn eval_let_shadows_outer_bindings_only_in_its_body() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(define x 10) (let ((x 2)) x)"),
            Ok(Val::Int(2))
        );
        assert_eq!(vm.eval_str(vm.global_module(), "x"), Ok(Val::Int(10)));
    }

    #[test]
    fn eval_let_in_lambda_reserves_slots_after_arguments() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "((lambda (x) (let ((y 3)) (+ x y))) 4)"),
            Ok(Val::Int(7))
        );
    }

    #[test]
    fn eval_let_allows_empty_bindings_and_multiple_body_expressions() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(let () 1 2)"),
            Ok(Val::Int(2))
        );
    }

    #[test]
    fn eval_recursive_function() {
        let mut vm = Vm::new();

        assert_eq!(
            vm.eval_str(
                vm.global_module(),
                r#"
(define (fib n)
  (if (< n 2)
    n
    (+ (fib (- n 1))
       (fib (- n 2)))))

(fib 10)"#
            ),
            Ok(Val::Int(55))
        );
    }

    #[test]
    fn eval_return_exits_early_with_value() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(begin (return 42) (+ #true))"),
            Ok(Val::Int(42))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(return)"),
            Ok(Val::Undefined)
        );
    }

    #[test]
    fn eval_str_returns_ast_errors() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "(")
            .expect_err("expected a parse error");

        assert!(matches!(error.kind, VmErrorKind::AstErrors { .. }));
    }

    #[test]
    fn eval_str_returns_compile_error_for_unbound_symbols() {
        let mut vm = Vm::new();
        let error = vm
            .eval_str(vm.global_module(), "missing")
            .expect_err("expected a compile error");
        assert!(matches!(
            error.kind,
            VmErrorKind::Compiler {
                error: CompilerError {
                    kind: CompilerErrorKind::UnboundSymbol(symbol),
                    ..
                },
                ..
            } if symbol == Symbol::must_inline("missing")
        ));
    }

    #[test]
    fn eval_if_selects_the_correct_branch() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(if #true 1 2)"),
            Ok(Val::Int(1))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(if #false 1 2)"),
            Ok(Val::Int(2))
        );
        // Unbound symbols are compile errors even in branches that are not selected.
        assert!(
            vm.eval_str(vm.global_module(), "(if #true missing 2)")
                .is_err()
        );
        assert!(
            vm.eval_str(vm.global_module(), "(if #false 1 missing)")
                .is_err()
        );
    }

    #[test]
    fn eval_if_treats_only_false_as_falsy() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(if 0 1 2)"),
            Ok(Val::Int(1))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(if nil 1 2)"),
            Ok(Val::Int(1))
        );
    }

    #[test]
    fn eval_if_without_else_returns_undefined() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(if #false 1)"),
            Ok(Val::Undefined)
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(if #true 1)"),
            Ok(Val::Int(1))
        );
    }

    #[test]
    fn eval_when_evaluates_all_body_expressions_when_true() {
        let mut vm = Vm::new();
        assert_eq!(
            vm.eval_str(vm.global_module(), "(when #true 1 2)"),
            Ok(Val::Int(2))
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(when #false (+ #true))"),
            Ok(Val::Undefined)
        );
        assert_eq!(
            vm.eval_str(vm.global_module(), "(when #true)"),
            Ok(Val::Undefined)
        );
    }
}
