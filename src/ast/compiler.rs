use crate::{
    ast::{Ast, AstKind},
    error::{CompilerError, CompilerErrorKind},
    function::{Arity, BytecodeFunction, CaptureSource, Instruction, UpvalueSlot},
    heap::{Heap, HeapHandle},
    module::Module,
    symbol::Symbol,
    tokenizer::Span,
    val::Val,
};

#[cfg(test)]
use crate::module::ModuleSource;

/// Compiles an AST into bytecode using a heap and module for runtime values.
///
/// The global module is treated as immutable: its values are inlined as
/// constants instead of being looked up at runtime.
pub struct CompilerContext<'a> {
    heap: &'a mut Heap,
    module: HeapHandle<Module>,
    global_module: HeapHandle<Module>,
    module_backup: Option<Module>,
    builder: BytecodeBuilder,
    parents: Vec<BytecodeBuilder>,
}

impl<'a> CompilerContext<'a> {
    /// Create a compiler context backed by the given heap, module, and global module.
    pub fn new(
        heap: &'a mut Heap,
        module: HeapHandle<Module>,
        global_module: HeapHandle<Module>,
    ) -> Self {
        CompilerContext {
            heap,
            module,
            global_module,
            module_backup: None,
            builder: BytecodeBuilder::default(),
            parents: Vec::new(),
        }
    }
}

impl<'a> CompilerContext<'a> {
    /// Compile an AST into a bytecode function.
    pub fn compile(mut self, ast: &Ast<'_>) -> Result<BytecodeFunction, CompilerError> {
        let result = self.compile_inner(ast);
        if result.is_err()
            && let Some(backup) = self.module_backup.take()
            && let Some(module) = self.heap.modules.get_mut(self.module)
        {
            *module = backup;
        }
        result
    }

    fn compile_inner(&mut self, ast: &Ast<'_>) -> Result<BytecodeFunction, CompilerError> {
        self.compile_ast(None, ast, true)?;
        if !self.builder.captures.is_empty() {
            return Err(CompilerError::new(
                "top-level compilation produced upvalues",
                ast.span.clone(),
            ));
        }
        let bytecode = self.builder.build(self.module, ast.span.clone())?;
        Ok(bytecode)
    }

    fn compile_ast(
        &mut self,
        name: Option<Symbol>,
        ast: &Ast<'_>,
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        match &ast.kind {
            AstKind::Constant(val) => self.emit_constant(*val, return_expression),
            AstKind::Lookup { namespace, symbol } => {
                if let Some(namespace) = namespace {
                    return Err(CompilerError {
                        kind: CompilerErrorKind::UnsupportedNamespace(*namespace),
                        span: ast.span.clone(),
                    });
                }
                self.emit_lookup(*symbol, ast.span.clone(), return_expression)?;
            }
            AstKind::Set { symbol, value } => {
                self.compile_ast(None, value, false)?;
                self.emit_set(*symbol, ast.span.clone(), return_expression)?;
            }
            AstKind::Define { symbol, value } => {
                self.compile_ast(Some(*symbol), value, false)?;
                let module = self.heap.modules.get_mut(self.module).unwrap();
                let slot = if let Some(slot) = module.slot(*symbol) {
                    slot
                } else {
                    self.module_backup.get_or_insert_with(|| module.clone());
                    module.define(*symbol)
                };
                self.emit(Instruction::Define(slot));
                if return_expression {
                    self.emit(Instruction::Return);
                }
            }
            AstKind::Lambda { args, body } => {
                self.compile_lambda(name, args, body, return_expression)?
            }
            AstKind::Let { bindings, body } => {
                self.compile_let(bindings, body, return_expression)?
            }
            AstKind::FunctionCall { function, args } => {
                self.compile_ast(None, function, false)?;
                for arg in args.iter() {
                    self.compile_ast(None, arg, false)?;
                }
                self.emit(Instruction::Eval(args.len()));
                if return_expression {
                    self.emit(Instruction::Return);
                }
            }
            AstKind::Begin(exprs) => {
                self.compile_begin(exprs, return_expression)?;
            }
            AstKind::Return(value) => {
                self.compile_ast(None, value, true)?;
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.compile_if(condition, then_branch, else_branch, return_expression)?;
            }
        }
        Ok(())
    }

    fn compile_let(
        &mut self,
        bindings: &[crate::ast::LetBinding<'_>],
        body: &Ast<'_>,
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        let scope_start = self.builder.local_names.len();
        for binding in bindings {
            self.compile_ast(None, binding.value, false)?;
            let index = self.builder.local_count;
            self.builder.local_count += 1;
            self.builder.local_names.push((binding.symbol, index));
            self.emit(Instruction::SetLocal(self.builder.arg_names.len() + index));
        }
        self.compile_ast(None, body, return_expression)?;
        self.builder.local_names.truncate(scope_start);
        Ok(())
    }

    fn compile_if(
        &mut self,
        condition: &Ast<'_>,
        then_branch: &Ast<'_>,
        else_branch: &Ast<'_>,
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        self.compile_ast(None, condition, false)?;
        let jump_to_else = self.builder.instructions.len();
        self.emit(Instruction::JumpIfFalse(0)); // placeholder

        self.compile_ast(None, then_branch, return_expression)?;
        if return_expression {
            // Both branches return directly, so no jump over the else branch is needed.
            let else_target = self.builder.instructions.len();
            self.builder.instructions[jump_to_else] = Instruction::JumpIfFalse(else_target);
            self.compile_ast(None, else_branch, true)?;
        } else {
            let jump_to_end = self.builder.instructions.len();
            self.emit(Instruction::Jump(0)); // placeholder

            let else_target = self.builder.instructions.len();
            self.builder.instructions[jump_to_else] = Instruction::JumpIfFalse(else_target);
            self.compile_ast(None, else_branch, false)?;

            let end_target = self.builder.instructions.len();
            self.builder.instructions[jump_to_end] = Instruction::Jump(end_target);
        }
        Ok(())
    }

    fn compile_begin(
        &mut self,
        exprs: &[Ast<'_>],
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        match exprs {
            [] => self.emit_constant(Val::Undefined, return_expression),
            [expr] => self.compile_ast(None, expr, return_expression)?,
            [leading_exprs @ .., final_expr] => {
                for expr in leading_exprs {
                    self.compile_ast(None, expr, false)?;
                }
                if !return_expression {
                    // Instruction not needed for tail call as we will return immediately.
                    self.emit(Instruction::PopN(leading_exprs.len()));
                }
                self.compile_ast(None, final_expr, return_expression)?;
            }
        }
        Ok(())
    }

    fn compile_lambda(
        &mut self,
        name: Option<Symbol>,
        args: &[Symbol],
        body: &Ast<'_>,
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        self.parents.push(std::mem::replace(
            &mut self.builder,
            BytecodeBuilder {
                name,
                arg_names: args.into(),
                local_names: Vec::new(),
                local_count: 0,
                instructions: Vec::new(),
                captures: Vec::new(),
            },
        ));
        self.compile_ast(None, body, true)?;
        let restored_builder = self.parents.pop().ok_or_else(|| {
            CompilerError::new(
                "lambda compilation lost its enclosing builder",
                body.span.clone(),
            )
        })?;
        let lambda_builder = std::mem::replace(&mut self.builder, restored_builder);
        let is_closure = !lambda_builder.captures.is_empty();
        let lambda_bytecode = lambda_builder.build(self.module, body.span.clone())?;
        let lambda_handle = self.heap.bytecode_functions.insert(lambda_bytecode);
        if is_closure {
            self.emit(Instruction::MakeClosure(lambda_handle));
        } else {
            self.emit(Instruction::PushConst(lambda_handle.into()));
        }
        if return_expression {
            self.emit(Instruction::Return);
        }
        Ok(())
    }

    fn emit(&mut self, instruction: Instruction) {
        self.builder.instructions.push(instruction);
    }

    fn emit_constant(&mut self, val: Val, return_expression: bool) {
        self.emit(Instruction::PushConst(val));
        if return_expression {
            self.emit(Instruction::Return);
        }
    }

    fn emit_lookup(
        &mut self,
        symbol: Symbol,
        span: Span,
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        let instruction = if let Some((_, index)) = self
            .builder
            .local_names
            .iter()
            .rev()
            .find(|(variable_name, _)| variable_name == &symbol)
        {
            Instruction::GetLocal(self.builder.arg_names.len() + index)
        } else if let Some(index) = self
            .builder
            .arg_names
            .iter()
            .rposition(|variable_name| variable_name == &symbol)
        {
            Instruction::GetLocal(index)
        } else if Some(symbol) == self.builder.name {
            Instruction::GetCurrentFunction
        } else if let Some(upvalue_slot) = self.builder.capture_upvalue(&mut self.parents, symbol) {
            Instruction::GetUpvalue(upvalue_slot)
        } else if let Some(slot) = self.heap.modules.get(self.module).unwrap().slot(symbol) {
            Instruction::LookupSlot(slot)
        } else {
            // The global module is immutable, so its values can be inlined as
            // constants instead of looked up at runtime.
            let Some(global_module) = self.heap.modules.get(self.global_module) else {
                std::hint::cold_path();
                return Err(CompilerError::new(
                    "global module is missing from the heap",
                    span,
                ));
            };
            let val = global_module.get(symbol);
            if val.is_undefined() {
                std::hint::cold_path();
                return Err(CompilerError::unbound_symbol(symbol, span));
            }
            Instruction::PushConst(val)
        };

        self.emit(instruction);
        if return_expression {
            self.emit(Instruction::Return)
        }
        Ok(())
    }

    fn emit_set(
        &mut self,
        symbol: Symbol,
        span: Span,
        return_expression: bool,
    ) -> Result<(), CompilerError> {
        let instruction = if let Some((_, index)) = self
            .builder
            .local_names
            .iter()
            .rev()
            .find(|(variable_name, _)| variable_name == &symbol)
        {
            Instruction::SetLocal(self.builder.arg_names.len() + index)
        } else if let Some(index) = self
            .builder
            .arg_names
            .iter()
            .rposition(|variable_name| variable_name == &symbol)
        {
            Instruction::SetLocal(index)
        } else if Some(symbol) == self.builder.name {
            return Err(CompilerError::immutable_symbol(symbol, span));
        } else if let Some(upvalue_slot) = self.builder.capture_upvalue(&mut self.parents, symbol) {
            if self
                .builder
                .upvalue_is_immutable(upvalue_slot, &self.parents)
            {
                return Err(CompilerError::immutable_symbol(symbol, span));
            }
            Instruction::SetUpvalue(upvalue_slot)
        } else if let Some(slot) = self.heap.modules.get(self.module).unwrap().slot(symbol) {
            Instruction::Define(slot)
        } else {
            let global_module = self.heap.modules.get(self.global_module).ok_or_else(|| {
                CompilerError::new("global module is missing from the heap", span.clone())
            })?;
            return Err(if global_module.get(symbol).is_undefined() {
                CompilerError::unbound_symbol(symbol, span)
            } else {
                CompilerError::immutable_symbol(symbol, span)
            });
        };

        self.emit(instruction);
        // SetLocal also initializes let bindings and consumes its operand.
        // Assignment expressions must leave a result like Define and SetUpvalue.
        if matches!(instruction, Instruction::SetLocal(_)) {
            self.emit(Instruction::PushConst(Val::Undefined));
        }
        if return_expression {
            self.emit(Instruction::Return)
        }
        Ok(())
    }
}

#[derive(Default)]
struct BytecodeBuilder {
    /// The name of the function.
    pub name: Option<Symbol>,
    /// The names of the function's arguments, in argument order.
    pub arg_names: Box<[Symbol]>,
    /// The currently visible let bindings, as `(name, local slot)` pairs.
    ///
    /// The slot is stored explicitly because `local_count` is never reused, so
    /// the position in this list can drift from the slot index once an inner
    /// `let` scope is truncated.
    pub local_names: Vec<(Symbol, usize)>,
    /// The instructions emitted for the function.
    pub instructions: Vec<Instruction>,
    /// The total number of local slots reserved by this function.
    pub local_count: usize,
    /// Values captured from the enclosing function.
    pub captures: Vec<CaptureDefinition>,
}

struct CaptureDefinition {
    symbol: Symbol,
    source: CaptureSource,
}

impl BytecodeBuilder {
    fn upvalue_is_immutable(&self, slot: UpvalueSlot, parents: &[BytecodeBuilder]) -> bool {
        let mut source = self.captures[slot.index()].source;
        for parent in parents.iter().rev() {
            match source {
                CaptureSource::Upvalue(index) => source = parent.captures[index].source,
                CaptureSource::CurrentFunction => return true,
                CaptureSource::Local(_) => return false,
            }
        }
        matches!(source, CaptureSource::CurrentFunction)
    }

    fn capture_upvalue(
        &mut self,
        parents: &mut [BytecodeBuilder],
        symbol: Symbol,
    ) -> Option<UpvalueSlot> {
        if let Some(index) = self
            .captures
            .iter()
            .position(|found| found.symbol == symbol)
        {
            return Some(UpvalueSlot::new(index));
        }

        let source = Self::ensure_upvalue(parents, symbol)?;
        let index = self.captures.len();
        self.captures.push(CaptureDefinition { symbol, source });
        Some(UpvalueSlot::new(index))
    }

    fn ensure_upvalue(parents: &mut [BytecodeBuilder], symbol: Symbol) -> Option<CaptureSource> {
        let (parent, ancestors) = parents.split_last_mut()?;
        if let Some((_, index)) = parent
            .local_names
            .iter()
            .rev()
            .find(|(variable_name, _)| variable_name == &symbol)
        {
            return Some(CaptureSource::Local(parent.arg_names.len() + index));
        }
        if let Some(index) = parent
            .arg_names
            .iter()
            .rposition(|variable_name| variable_name == &symbol)
        {
            return Some(CaptureSource::Local(index));
        }
        if parent.name == Some(symbol) {
            return Some(CaptureSource::CurrentFunction);
        }
        if let Some(index) = parent
            .captures
            .iter()
            .position(|found| found.symbol == symbol)
        {
            return Some(CaptureSource::Upvalue(index));
        }
        if ancestors.is_empty() {
            return None;
        }

        let source = Self::ensure_upvalue(ancestors, symbol)?;
        let index = parent.captures.len();
        parent.captures.push(CaptureDefinition { symbol, source });
        Some(CaptureSource::Upvalue(index))
    }

    fn build(
        &self,
        module: HeapHandle<Module>,
        span: Span,
    ) -> Result<BytecodeFunction, CompilerError> {
        let arity = u32::try_from(self.arg_names.len())
            .map_err(|_| CompilerError::new("function arity exceeds u32::MAX", span.clone()))?;
        let local_count = u32::try_from(self.local_count)
            .map_err(|_| CompilerError::new("local variable count exceeds u32::MAX", span))?;
        let bytecode = BytecodeFunction {
            module,
            name: self.name,
            arity: Arity::Exact(arity),
            local_count,
            captures: self
                .captures
                .iter()
                .map(|upvalue| upvalue.source)
                .collect::<Vec<_>>()
                .into(),
            bytecode: self.instructions.as_slice().into(),
        };
        Ok(bytecode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_map::SourceId;
    use bumpalo::Bump;

    #[test]
    fn invalid_assignments_return_compile_errors() {
        use crate::error::CompilerErrorKind;

        let symbol = Symbol::must_inline("target");
        for (function_name, global_value, expected) in [
            (None, None, CompilerErrorKind::UnboundSymbol(symbol)),
            (
                Some(symbol),
                None,
                CompilerErrorKind::ImmutableSymbol(symbol),
            ),
            (
                None,
                Some(Val::Int(42)),
                CompilerErrorKind::ImmutableSymbol(symbol),
            ),
        ] {
            let mut heap = Heap::default();
            let global_module = heap.modules.insert(ModuleSource::Global).unwrap();
            if let Some(value) = global_value {
                heap.modules
                    .get_mut(global_module)
                    .unwrap()
                    .set(symbol, value);
            }
            let module = heap.modules.insert(ModuleSource::Internal("test")).unwrap();
            let value = Ast {
                kind: AstKind::Constant(Val::Int(1)),
                span: 12..13,
            };
            let assignment = Ast {
                kind: AstKind::Set {
                    symbol,
                    value: &value,
                },
                span: 0..14,
            };
            let mut compiler = CompilerContext::new(&mut heap, module, global_module);
            compiler.builder.name = function_name;
            assert_eq!(
                compiler.compile(&assignment).unwrap_err(),
                CompilerError {
                    kind: expected,
                    span: 0..14
                },
            );
            assert!(heap.modules.get(module).unwrap().slot(symbol).is_none());
            if let Some(value) = global_value {
                assert_eq!(heap.modules.get(global_module).unwrap().get(symbol), value);
            }
        }
    }

    #[test]
    fn nested_functions_retain_the_compilation_module() {
        let mut heap = Heap::default();
        let global_module = heap.modules.insert(ModuleSource::Global).unwrap();
        let _other_module = heap
            .modules
            .insert(ModuleSource::Internal("other"))
            .unwrap();
        let module = heap.modules.insert(ModuleSource::Internal("test")).unwrap();
        let bump = Bump::new();
        let ast = Ast::parse(
            SourceId::new_unique(),
            "(lambda (x) (lambda () x))",
            &bump,
            &mut heap,
        )
        .unwrap();
        let top = CompilerContext::new(&mut heap, module, global_module)
            .compile(&ast)
            .unwrap();
        assert_eq!(top.module, module);
        let Instruction::PushConst(Val::BytecodeFunction(outer)) = top.bytecode[0] else {
            panic!("expected lambda constant");
        };
        let outer = heap.bytecode_functions.get(outer).unwrap();
        assert_eq!(outer.module, module);
        let Instruction::MakeClosure(inner) = outer.bytecode[0] else {
            panic!("expected nested closure");
        };
        assert_eq!(heap.bytecode_functions.get(inner).unwrap().module, module);
    }

    #[test]
    fn module_backup_is_created_only_when_adding_a_slot() {
        let mut heap = Heap::default();
        let module = heap.modules.insert(ModuleSource::Internal("test")).unwrap();
        let existing = Symbol::must_inline("prior");
        heap.modules
            .get_mut(module)
            .unwrap()
            .set(existing, Val::Int(42));
        let bump = Bump::new();
        let ast = Ast::parse(
            SourceId::new_unique(),
            "(begin prior (define prior 43))",
            &bump,
            &mut heap,
        )
        .unwrap();
        let global_module = heap.modules.insert(ModuleSource::Global).unwrap();
        let mut compiler = CompilerContext::new(&mut heap, module, global_module);
        compiler.compile_inner(&ast).unwrap();
        assert!(compiler.module_backup.is_none());

        let ast = Ast::parse(
            SourceId::new_unique(),
            "(begin (define first 1) (define second 2))",
            &bump,
            compiler.heap,
        )
        .unwrap();
        compiler.compile_inner(&ast).unwrap();
        let backup = compiler.module_backup.as_ref().unwrap();
        assert_eq!(backup.get(existing), Val::Int(42));
        assert!(backup.slot(Symbol::must_inline("first")).is_none());
        assert!(backup.slot(Symbol::must_inline("second")).is_none());
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn compilation_failure_restores_module_slots_and_values() {
        let mut heap = Heap::default();
        let module = heap.modules.insert(ModuleSource::Internal("test")).unwrap();
        let existing = Symbol::must_inline("prior");
        heap.modules
            .get_mut(module)
            .unwrap()
            .set(existing, Val::Int(42));
        let original_slot = heap.modules.get(module).unwrap().slot(existing);
        let bump = Bump::new();
        let ast = Ast::parse(
            SourceId::new_unique(),
            "(begin (define first 1) (define second 2))",
            &bump,
            &mut heap,
        )
        .unwrap();
        let global_module = heap.modules.insert(ModuleSource::Global).unwrap();
        let mut compiler = CompilerContext::new(&mut heap, module, global_module);
        // Force bytecode construction to fail after both definitions compile.
        compiler.builder.local_count = usize::MAX;
        assert!(compiler.compile(&ast).is_err());

        let module = heap.modules.get(module).unwrap();
        assert_eq!(module.slot(existing), original_slot);
        assert_eq!(module.get(existing), Val::Int(42));
        assert!(module.slot(Symbol::must_inline("first")).is_none());
        assert!(module.slot(Symbol::must_inline("second")).is_none());
    }
}
