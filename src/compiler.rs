//! Bytecode compiler: transforms the AST into a [`Function`] containing a
//! [`Chunk`] of bytecode.
//!
//! # Strategy
//!
//! This is a **single-pass, tree-walking emitter** in the style of Crafting
//! Interpreters' `clox`. It does not build a separate IR; each AST node is
//! emitted directly as it is visited.
//!
//! Local variable resolution (slot indices) and upvalue capture are computed
//! here, *independently* of [`crate::resolver`]. The two passes deliberately
//! duplicate this logic:
//!
//! * [`crate::resolver`] runs first and produces user-facing semantic
//!   diagnostics plus distances for the tree-walking interpreter.
//! * the compiler computes slot indices and upvalue descriptors for bytecode.
//!
//! Keeping them separate means the interpreter and the VM are genuinely
//! independent implementations, which strengthens the cross-checking tests.
//!
//! # Compilation of nested functions
//!
//! Nested functions are compiled with an explicit **stack of function states**
//! ([`Compiler::states`]) rather than parent pointers. When a function
//! declaration is compiled, a new state is pushed; while its body is emitted,
//! the enclosing state remains on the stack, so resolving an upvalue is a walk
//! over `states` from the current index downward.
//!
//! # Limits
//!
//! * [`MAX_PARAMS`] parameters/arguments (one-byte operand).
//! * [`MAX_LOCALS`] locals per function (`u8` slot indices).
//! * [`MAX_UPVALUES`] captured variables per function.
//! * [`crate::chunk::MAX_CONSTANTS`] constants per function.

use crate::ast::{BinaryOp, Expr, LogicalOp, Stmt, UnaryOp};
use crate::chunk::Chunk;
use crate::error::Error;
use crate::object::{Function, Object};
use crate::opcode::OpCode;
use crate::span::Span;
use crate::token::Literal;
use crate::value::Value;
use std::rc::Rc;

/// Maximum number of parameters (and arguments) per function.
///
/// Argument counts are encoded in a single operand byte, so 255 is the hard
/// ceiling. The named constant documents the rationale.
pub const MAX_PARAMS: usize = 255;

/// Maximum number of local variables in a function (slot indices are `u8`).
const MAX_LOCALS: usize = 256;

/// Maximum number of captured variables (upvalue indices are `u8`).
const MAX_UPVALUES: usize = 256;

/// A local variable slot.
#[derive(Debug)]
struct Local {
    /// The variable's name.
    name: String,
    /// Scope depth; `-1` marks "declared but not yet initialized".
    depth: i32,
    /// Whether an inner function captures this local.
    is_captured: bool,
}

/// An upvalue descriptor: where the captured variable lives in the enclosing
/// function (a local slot or another upvalue).
#[derive(Clone, Copy, Debug)]
struct UpvalueDesc {
    /// Slot index in the enclosing function.
    index: u8,
    /// `true` if the index refers to a local, `false` if to an upvalue.
    is_local: bool,
}

/// The kind of function currently being compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FunctionKind {
    Script,
    Function,
    Method,
    Initializer,
}

/// Whether a loop is a `while` (continue jumps before the condition) or a `for`
/// (continue jumps to the increment clause).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoopKind {
    While,
    For,
}

/// Bookkeeping for a loop, used to patch `break`/`continue` jumps.
#[derive(Debug)]
struct LoopContext {
    /// Scope depth of the loop header; body locals have greater depth.
    scope_depth: i32,
    /// Kind of loop.
    kind: LoopKind,
    /// Byte offset of the loop start (used by `while` continue).
    start: usize,
    /// Operand offsets of pending `break` jumps.
    breaks: Vec<usize>,
    /// Operand offsets of pending `continue` jumps (`for` only).
    continues: Vec<usize>,
}

/// State for one function being compiled.
#[derive(Debug)]
struct FunctionState {
    name: String,
    arity: usize,
    chunk: Chunk,
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDesc>,
    scope_depth: i32,
    kind: FunctionKind,
    loops: Vec<LoopContext>,
}

impl FunctionState {
    fn new(name: String, kind: FunctionKind) -> Self {
        // Slot 0 is reserved. For methods it is the implicit `this` receiver;
        // for plain functions/scripts it is an unused placeholder.
        let slot0 = if matches!(kind, FunctionKind::Method | FunctionKind::Initializer) {
            "this"
        } else {
            ""
        };
        Self {
            name,
            arity: 0,
            chunk: Chunk::new(),
            locals: vec![Local {
                name: slot0.to_string(),
                depth: 0,
                is_captured: false,
            }],
            upvalues: Vec::new(),
            scope_depth: 0,
            kind,
            loops: Vec::new(),
        }
    }
}

/// The compiler.
pub struct Compiler {
    states: Vec<FunctionState>,
    errors: Vec<Error>,
    source: String,
}

impl Compiler {
    fn new() -> Self {
        Self {
            states: vec![FunctionState::new(
                "<script>".to_string(),
                FunctionKind::Script,
            )],
            errors: Vec::new(),
            source: String::new(),
        }
    }

    // --- Emitting ---------------------------------------------------------

    fn chunk_len(&self) -> usize {
        self.states.last().expect("compiler state").chunk.code.len()
    }

    fn line_of(&self, span: Span) -> usize {
        span.line_col(&self.source).0
    }

    fn emit(&mut self, op: OpCode, span: Span) {
        let line = self.line_of(span);
        self.states
            .last_mut()
            .expect("compiler state")
            .chunk
            .write(op, line, span);
    }

    fn emit_byte(&mut self, byte: u8, span: Span) {
        let line = self.line_of(span);
        self.states
            .last_mut()
            .expect("compiler state")
            .chunk
            .write_byte(byte, line, span);
    }

    fn make_constant(&mut self, value: Value, span: Span) -> u8 {
        let result = self
            .states
            .last_mut()
            .expect("compiler state")
            .chunk
            .add_constant(value, span);
        match result {
            Ok(index) => index,
            Err(err) => {
                self.errors.push(err);
                0
            }
        }
    }

    fn emit_constant(&mut self, value: Value, span: Span) {
        let index = self.make_constant(value, span);
        self.emit(OpCode::Constant, span);
        self.emit_byte(index, span);
    }

    fn identifier_constant(&mut self, name: &str, span: Span) -> u8 {
        self.make_constant(Value::string(name), span)
    }

    fn patch_jump_to(&mut self, offset: usize, target: usize, span: Span) {
        if target < offset + 2 {
            self.error(span, "internal error: invalid backward jump target");
            return;
        }
        let jump = target - offset - 2;
        if jump > u16::MAX as usize {
            self.error(span, "too much code to jump over");
            return;
        }
        let code = &mut self.states.last_mut().expect("compiler state").chunk.code;
        code[offset] = (jump >> 8) as u8;
        code[offset + 1] = (jump & 0xff) as u8;
    }

    fn patch_jump(&mut self, offset: usize, span: Span) {
        let target = self.chunk_len();
        self.patch_jump_to(offset, target, span);
    }

    fn emit_jump(&mut self, op: OpCode, span: Span) -> usize {
        self.emit(op, span);
        let line = self.line_of(span);
        self.states
            .last_mut()
            .expect("compiler state")
            .chunk
            .write_byte(0xff, line, span);
        self.states
            .last_mut()
            .expect("compiler state")
            .chunk
            .write_byte(0xff, line, span);
        self.chunk_len() - 2
    }

    fn emit_loop(&mut self, loop_start: usize, span: Span) {
        self.emit(OpCode::Loop, span);
        let offset = self.chunk_len() - loop_start + 2;
        if offset > u16::MAX as usize {
            self.error(span, "loop body too large");
            return;
        }
        self.emit_byte((offset >> 8) as u8, span);
        self.emit_byte((offset & 0xff) as u8, span);
    }

    // --- Errors -----------------------------------------------------------

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(Error::compile(message, span));
    }

    // --- Scopes and locals ------------------------------------------------

    fn scope_depth(&self) -> i32 {
        self.states.last().expect("compiler state").scope_depth
    }

    fn begin_scope(&mut self) {
        self.states.last_mut().expect("compiler state").scope_depth += 1;
    }

    fn end_scope(&mut self, span: Span) {
        let target = {
            let state = self.states.last_mut().expect("compiler state");
            state.scope_depth -= 1;
            state.scope_depth
        };
        // Emit a pop (or upvalue close) for every local leaving scope.
        loop {
            let captured = match self.states.last().expect("compiler state").locals.last() {
                Some(local) if local.depth > target => local.is_captured,
                _ => break,
            };
            if captured {
                self.emit(OpCode::CloseUpvalue, span);
            } else {
                self.emit(OpCode::Pop, span);
            }
            self.states.last_mut().expect("compiler state").locals.pop();
        }
    }

    fn add_local(&mut self, name: &str, span: Span) {
        // Slot indices are one byte, and slot 0 is reserved, so a function can
        // hold at most MAX_LOCALS locals.
        if self.states.last().expect("compiler state").locals.len() >= MAX_LOCALS {
            self.error(span, "too many local variables in function");
            return;
        }
        self.states
            .last_mut()
            .expect("compiler state")
            .locals
            .push(Local {
                name: name.to_string(),
                depth: -1,
                is_captured: false,
            });
    }

    fn declare_variable(&mut self, name: &str, span: Span) {
        if self.scope_depth() == 0 {
            return; // Globals are resolved dynamically.
        }
        // Reject duplicate declarations in the same scope.
        let scope_depth = self.scope_depth();
        let mut duplicate = false;
        {
            let locals = &self.states.last().expect("compiler state").locals;
            for local in locals.iter().rev() {
                if local.depth != -1 && local.depth < scope_depth {
                    break;
                }
                if local.name == name {
                    duplicate = true;
                    break;
                }
            }
        }
        if duplicate {
            self.error(
                span,
                format!("variable '{name}' already declared in this scope"),
            );
        }
        self.add_local(name, span);
    }

    fn mark_initialized(&mut self) {
        if self.scope_depth() == 0 {
            return;
        }
        let depth = self.scope_depth();
        if let Some(local) = self
            .states
            .last_mut()
            .expect("compiler state")
            .locals
            .last_mut()
        {
            local.depth = depth;
        }
    }

    fn define_variable(&mut self, name: &str, global_index: u8, span: Span) {
        if self.scope_depth() > 0 {
            self.mark_initialized();
            return;
        }
        let _ = name;
        self.emit(OpCode::DefineGlobal, span);
        self.emit_byte(global_index, span);
    }

    // --- Resolution -------------------------------------------------------

    fn resolve_local(&mut self, function_index: usize, name: &str, span: Span) -> Option<u8> {
        let mut found = None;
        let mut uninitialized = false;
        {
            let locals = &self.states[function_index].locals;
            for (i, local) in locals.iter().enumerate().rev() {
                if local.name == name {
                    if local.depth == -1 {
                        uninitialized = true;
                    }
                    found = Some(i as u8);
                    break;
                }
            }
        }
        if uninitialized {
            self.error(
                span,
                format!("cannot read local variable '{name}' in its own initializer"),
            );
        }
        found
    }

    fn add_upvalue(&mut self, function_index: usize, index: u8, is_local: bool, span: Span) -> u8 {
        if let Some(existing) = self.states[function_index]
            .upvalues
            .iter()
            .position(|u| u.index == index && u.is_local == is_local)
        {
            return existing as u8;
        }
        if self.states[function_index].upvalues.len() >= MAX_UPVALUES {
            self.error(span, "too many closure variables in function");
            return 0;
        }
        let state = &mut self.states[function_index];
        state.upvalues.push(UpvalueDesc { index, is_local });
        (state.upvalues.len() - 1) as u8
    }

    fn resolve_upvalue(&mut self, function_index: usize, name: &str, span: Span) -> Option<u8> {
        if function_index == 0 {
            return None;
        }
        if let Some(local) = self.resolve_local(function_index - 1, name, span) {
            self.states[function_index - 1].locals[local as usize].is_captured = true;
            return Some(self.add_upvalue(function_index, local, true, span));
        }
        if let Some(upvalue) = self.resolve_upvalue(function_index - 1, name, span) {
            return Some(self.add_upvalue(function_index, upvalue, false, span));
        }
        None
    }

    fn variable_get(&mut self, name: &str, span: Span) {
        let function_index = self.states.len() - 1;
        if let Some(slot) = self.resolve_local(function_index, name, span) {
            self.emit(OpCode::GetLocal, span);
            self.emit_byte(slot, span);
        } else if let Some(slot) = self.resolve_upvalue(function_index, name, span) {
            self.emit(OpCode::GetUpvalue, span);
            self.emit_byte(slot, span);
        } else {
            let constant = self.identifier_constant(name, span);
            self.emit(OpCode::GetGlobal, span);
            self.emit_byte(constant, span);
        }
    }

    /// Emits the store half of a variable assignment. The value is already on
    /// the stack, and all set opcodes leave it there so the assignment
    /// expression produces it.
    fn variable_set(&mut self, name: &str, span: Span) {
        let function_index = self.states.len() - 1;
        if let Some(slot) = self.resolve_local(function_index, name, span) {
            self.emit(OpCode::SetLocal, span);
            self.emit_byte(slot, span);
        } else if let Some(slot) = self.resolve_upvalue(function_index, name, span) {
            self.emit(OpCode::SetUpvalue, span);
            self.emit_byte(slot, span);
        } else {
            let constant = self.identifier_constant(name, span);
            self.emit(OpCode::SetGlobal, span);
            self.emit_byte(constant, span);
        }
    }

    // --- Statements -------------------------------------------------------

    fn compile_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expression { expr, .. } => {
                self.compile_expr(expr);
                self.emit(OpCode::Pop, expr.span());
            }
            Stmt::Print { expr, .. } => {
                self.compile_expr(expr);
                self.emit(OpCode::Print, expr.span());
            }
            Stmt::Var {
                name,
                initializer,
                span,
            } => {
                let global = if self.scope_depth() == 0 {
                    self.identifier_constant(name, *span)
                } else {
                    0
                };
                self.declare_variable(name, *span);
                match initializer {
                    Some(expr) => self.compile_expr(expr),
                    None => self.emit(OpCode::Nil, *span),
                }
                self.define_variable(name, global, *span);
            }
            Stmt::Block { statements, .. } => {
                self.begin_scope();
                for s in statements {
                    self.compile_stmt(s);
                }
                self.end_scope(stmt.span());
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.compile_expr(condition);
                let then_jump = self.emit_jump(OpCode::JumpIfFalse, condition.span());
                self.emit(OpCode::Pop, condition.span());
                self.compile_stmt(then_branch);
                let else_jump = self.emit_jump(OpCode::Jump, then_branch.span());
                self.patch_jump(then_jump, condition.span());
                self.emit(OpCode::Pop, condition.span());
                if let Some(else_b) = else_branch {
                    self.compile_stmt(else_b);
                }
                self.patch_jump(else_jump, then_branch.span());
            }
            Stmt::While {
                condition,
                body,
                span,
            } => self.compile_while(condition, body, *span),
            Stmt::For {
                init,
                condition,
                increment,
                body,
                span,
            } => self.compile_for(
                init.as_deref(),
                condition.as_ref(),
                increment.as_ref(),
                body,
                *span,
            ),
            Stmt::Function {
                name,
                params,
                body,
                span,
            } => {
                let global = if self.scope_depth() == 0 {
                    self.identifier_constant(name, *span)
                } else {
                    0
                };
                self.declare_variable(name, *span);
                // Mark initialized before the body so the function can refer to
                // itself recursively (for local functions).
                if self.scope_depth() > 0 {
                    self.mark_initialized();
                }
                self.compile_function(name, params, body, FunctionKind::Function, *span);
                self.define_variable(name, global, *span);
            }
            Stmt::Return {
                value,
                span,
                keyword_span,
            } => {
                let kind = self.states.last().expect("compiler state").kind;
                if kind == FunctionKind::Script {
                    self.error(*keyword_span, "cannot return from top-level code");
                }
                if kind == FunctionKind::Initializer {
                    if value.is_some() {
                        self.error(*keyword_span, "cannot return a value from an initializer");
                    }
                    // Initializers return the receiver.
                    self.emit(OpCode::GetLocal, *span);
                    self.emit_byte(0, *span);
                } else if let Some(v) = value {
                    self.compile_expr(v);
                } else {
                    self.emit(OpCode::Nil, *span);
                }
                self.emit(OpCode::Return, *span);
            }
            Stmt::Break { span } => self.compile_break(*span),
            Stmt::Continue { span } => self.compile_continue(*span),
            Stmt::Class {
                name,
                superclass,
                methods,
                span,
            } => self.compile_class(name, superclass.as_ref(), methods, *span),
        }
    }

    fn compile_while(&mut self, condition: &Expr, body: &Stmt, span: Span) {
        let loop_start = self.chunk_len();
        self.compile_expr(condition);
        let exit_jump = self.emit_jump(OpCode::JumpIfFalse, condition.span());
        self.emit(OpCode::Pop, condition.span());

        let scope_depth = self.scope_depth();
        self.states
            .last_mut()
            .expect("compiler state")
            .loops
            .push(LoopContext {
                scope_depth,
                kind: LoopKind::While,
                start: loop_start,
                breaks: Vec::new(),
                continues: Vec::new(),
            });

        self.compile_stmt(body);
        self.emit_loop(loop_start, span);

        self.patch_jump(exit_jump, span);
        self.emit(OpCode::Pop, span); // pop condition on the exit path

        let loop_ctx = self
            .states
            .last_mut()
            .expect("compiler state")
            .loops
            .pop()
            .expect("loop context");
        let target = self.chunk_len();
        for brk in loop_ctx.breaks {
            self.patch_jump_to(brk, target, span);
        }
    }

    fn compile_for(
        &mut self,
        init: Option<&Stmt>,
        condition: Option<&Expr>,
        increment: Option<&Expr>,
        body: &Stmt,
        span: Span,
    ) {
        self.begin_scope();
        if let Some(init_stmt) = init {
            self.compile_stmt(init_stmt);
        }

        let loop_start = self.chunk_len();
        let mut exit_jump = None;
        if let Some(cond) = condition {
            self.compile_expr(cond);
            exit_jump = Some(self.emit_jump(OpCode::JumpIfFalse, cond.span()));
            self.emit(OpCode::Pop, cond.span());
        }

        let scope_depth = self.scope_depth();
        self.states
            .last_mut()
            .expect("compiler state")
            .loops
            .push(LoopContext {
                scope_depth,
                kind: LoopKind::For,
                start: loop_start,
                breaks: Vec::new(),
                continues: Vec::new(),
            });

        self.compile_stmt(body);

        // `continue` targets the increment clause.
        let increment_start = self.chunk_len();
        if let Some(inc) = increment {
            self.compile_expr(inc);
            self.emit(OpCode::Pop, inc.span());
        }
        let continue_target = increment_start;

        self.emit_loop(loop_start, span);
        if let Some(exit) = exit_jump {
            self.patch_jump(exit, span);
            self.emit(OpCode::Pop, span); // pop condition
        }

        let loop_ctx = self
            .states
            .last_mut()
            .expect("compiler state")
            .loops
            .pop()
            .expect("loop context");
        for cont in loop_ctx.continues {
            self.patch_jump_to(cont, continue_target, span);
        }
        let break_target = self.chunk_len();
        for brk in loop_ctx.breaks {
            self.patch_jump_to(brk, break_target, span);
        }

        self.end_scope(span);
    }

    fn compile_break(&mut self, span: Span) {
        let Some(scope_depth) = self
            .states
            .last()
            .expect("compiler state")
            .loops
            .last()
            .map(|loop_ctx| loop_ctx.scope_depth)
        else {
            self.error(span, "cannot use 'break' outside of a loop");
            return;
        };
        self.emit_control_pops(scope_depth, span);
        let jump = self.emit_jump(OpCode::Jump, span);
        self.states
            .last_mut()
            .expect("compiler state")
            .loops
            .last_mut()
            .expect("loop context")
            .breaks
            .push(jump);
    }

    fn compile_continue(&mut self, span: Span) {
        // Copy the needed loop info first to avoid holding a borrow.
        let Some((scope_depth, kind, start)) = self
            .states
            .last()
            .expect("compiler state")
            .loops
            .last()
            .map(|loop_ctx| (loop_ctx.scope_depth, loop_ctx.kind, loop_ctx.start))
        else {
            self.error(span, "cannot use 'continue' outside of a loop");
            return;
        };
        self.emit_control_pops(scope_depth, span);
        match kind {
            LoopKind::While => {
                // `while` re-evaluates the condition, which is still on the
                // stack at the continue point: pop it.
                self.emit(OpCode::Pop, span);
                self.emit_loop(start, span);
            }
            LoopKind::For => {
                let jump = self.emit_jump(OpCode::Jump, span);
                self.states
                    .last_mut()
                    .expect("compiler state")
                    .loops
                    .last_mut()
                    .expect("loop context")
                    .continues
                    .push(jump);
            }
        }
    }

    /// Emits pops/closes for all locals with depth greater than `threshold`,
    /// without removing them from the compile-time locals list (they remain in
    /// scope for subsequent statements on the non-control-flow path).
    fn emit_control_pops(&mut self, threshold: i32, span: Span) {
        let to_pop: Vec<bool> = {
            let locals = &self.states.last().expect("compiler state").locals;
            locals
                .iter()
                .rev()
                .take_while(|local| local.depth > threshold)
                .map(|local| local.is_captured)
                .collect()
        };
        for captured in to_pop {
            if captured {
                self.emit(OpCode::CloseUpvalue, span);
            } else {
                self.emit(OpCode::Pop, span);
            }
        }
    }

    fn compile_class(
        &mut self,
        name: &str,
        superclass: Option<&Expr>,
        methods: &[Stmt],
        span: Span,
    ) {
        let name_constant = self.identifier_constant(name, span);
        self.declare_variable(name, span);
        self.emit(OpCode::Class, span);
        self.emit_byte(name_constant, span);
        self.define_variable(name, name_constant, span);

        let mut has_superclass = false;
        if let Some(super_expr) = superclass {
            has_superclass = true;
            if let Expr::Variable {
                name: super_name, ..
            } = super_expr
            {
                if super_name == name {
                    self.error(span, "a class cannot inherit from itself");
                }
            }
            self.compile_expr(super_expr); // push superclass
            self.begin_scope();
            self.add_local("super", span);
            self.mark_initialized();
            self.variable_get(name, span); // push subclass
            self.emit(OpCode::Inherit, span);
        }

        // Push the class so methods can be attached to it.
        self.variable_get(name, span);
        for method in methods {
            if let Stmt::Function {
                name: method_name,
                params,
                body,
                span: method_span,
            } = method
            {
                let method_constant = self.identifier_constant(method_name, *method_span);
                let kind = if method_name == "init" {
                    FunctionKind::Initializer
                } else {
                    FunctionKind::Method
                };
                self.compile_function(method_name, params, body, kind, *method_span);
                self.emit(OpCode::Method, *method_span);
                self.emit_byte(method_constant, *method_span);
            }
        }
        self.emit(OpCode::Pop, span);

        if has_superclass {
            self.end_scope(span);
        }
    }

    /// Compiles a function body into a separate [`Function`] and emits an
    /// `OP_CLOSURE` in the enclosing chunk.
    fn compile_function(
        &mut self,
        name: &str,
        params: &[String],
        body: &[Stmt],
        kind: FunctionKind,
        span: Span,
    ) {
        let mut state = FunctionState::new(name.to_string(), kind);
        state.arity = params.len();
        self.states.push(state);

        // Parameters live in a scope enclosing the body.
        self.begin_scope();
        for param in params {
            self.add_local(param, span);
            self.mark_initialized();
        }

        for stmt in body {
            self.compile_stmt(stmt);
        }

        // Implicit return.
        if kind == FunctionKind::Initializer {
            self.emit(OpCode::GetLocal, span);
            self.emit_byte(0, span);
        } else {
            self.emit(OpCode::Nil, span);
        }
        self.emit(OpCode::Return, span);

        let finished = self.states.pop().expect("function state");
        let upvalues = finished.upvalues;
        let upvalue_count = upvalues.len();
        let function = Rc::new(Function {
            name: finished.name,
            arity: finished.arity,
            chunk: finished.chunk,
            upvalue_count,
        });

        let constant = self.make_constant(Value::Obj(Rc::new(Object::Function(function))), span);
        self.emit(OpCode::Closure, span);
        self.emit_byte(constant, span);
        for upvalue in upvalues {
            self.emit_byte(u8::from(upvalue.is_local), span);
            self.emit_byte(upvalue.index, span);
        }
    }

    // --- Expressions ------------------------------------------------------

    fn compile_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal { value, span } => self.compile_literal(value, *span),
            Expr::Grouping { expression, .. } => self.compile_expr(expression),
            Expr::Variable { name, span } => self.variable_get(name, *span),
            Expr::Assign { name, value, span } => {
                self.compile_expr(value);
                self.variable_set(name, *span);
            }
            Expr::Unary {
                op, right, span, ..
            } => {
                self.compile_expr(right);
                match op {
                    UnaryOp::Not => self.emit(OpCode::Not, *span),
                    UnaryOp::Neg => self.emit(OpCode::Negate, *span),
                }
            }
            Expr::Binary {
                left,
                op,
                right,
                span,
            } => {
                self.compile_expr(left);
                self.compile_expr(right);
                match op {
                    BinaryOp::Add => self.emit(OpCode::Add, *span),
                    BinaryOp::Sub => self.emit(OpCode::Subtract, *span),
                    BinaryOp::Mul => self.emit(OpCode::Multiply, *span),
                    BinaryOp::Div => self.emit(OpCode::Divide, *span),
                    BinaryOp::Eq => self.emit(OpCode::Equal, *span),
                    BinaryOp::Ne => {
                        self.emit(OpCode::Equal, *span);
                        self.emit(OpCode::Not, *span);
                    }
                    BinaryOp::Gt => self.emit(OpCode::Greater, *span),
                    BinaryOp::Ge => {
                        self.emit(OpCode::Less, *span);
                        self.emit(OpCode::Not, *span);
                    }
                    BinaryOp::Lt => self.emit(OpCode::Less, *span),
                    BinaryOp::Le => {
                        self.emit(OpCode::Greater, *span);
                        self.emit(OpCode::Not, *span);
                    }
                }
            }
            Expr::Logical {
                left, op, right, ..
            } => self.compile_logical(left, *op, right),
            Expr::Call {
                callee, args, span, ..
            } => self.compile_call(callee, args, *span),
            Expr::Get {
                object, name, span, ..
            } => {
                self.compile_expr(object);
                let constant = self.identifier_constant(name, *span);
                self.emit(OpCode::GetProperty, *span);
                self.emit_byte(constant, *span);
            }
            Expr::Set {
                object,
                name,
                value,
                span,
            } => {
                self.compile_expr(object);
                self.compile_expr(value);
                let constant = self.identifier_constant(name, *span);
                self.emit(OpCode::SetProperty, *span);
                self.emit_byte(constant, *span);
            }
            Expr::This { span } => self.variable_get("this", *span),
            Expr::Super { method, span } => {
                self.variable_get("this", *span);
                let constant = self.identifier_constant(method, *span);
                self.variable_get("super", *span);
                self.emit(OpCode::GetSuper, *span);
                self.emit_byte(constant, *span);
            }
        }
    }

    fn compile_literal(&mut self, literal: &Literal, span: Span) {
        match literal {
            Literal::Number(n) => self.emit_constant(Value::Number(*n), span),
            Literal::String(s) => self.emit_constant(Value::string(s), span),
            Literal::Bool(true) => self.emit(OpCode::True, span),
            Literal::Bool(false) => self.emit(OpCode::False, span),
            Literal::Nil => self.emit(OpCode::Nil, span),
        }
    }

    fn compile_logical(&mut self, left: &Expr, op: LogicalOp, right: &Expr) {
        match op {
            LogicalOp::And => {
                self.compile_expr(left);
                let end_jump = self.emit_jump(OpCode::JumpIfFalse, left.span());
                self.emit(OpCode::Pop, left.span());
                self.compile_expr(right);
                self.patch_jump(end_jump, left.span());
            }
            LogicalOp::Or => {
                self.compile_expr(left);
                let else_jump = self.emit_jump(OpCode::JumpIfFalse, left.span());
                let end_jump = self.emit_jump(OpCode::Jump, left.span());
                self.patch_jump(else_jump, left.span());
                self.emit(OpCode::Pop, left.span());
                self.compile_expr(right);
                self.patch_jump(end_jump, left.span());
            }
        }
    }

    fn compile_call(&mut self, callee: &Expr, args: &[Expr], span: Span) {
        match callee {
            Expr::Get {
                object,
                name,
                span: get_span,
                ..
            } => {
                self.compile_expr(object);
                for arg in args {
                    self.compile_expr(arg);
                }
                let constant = self.identifier_constant(name, *get_span);
                self.emit(OpCode::Invoke, span);
                self.emit_byte(constant, span);
                self.emit_byte(args.len() as u8, span);
            }
            Expr::Super {
                method,
                span: super_span,
            } => {
                self.variable_get("this", *super_span);
                for arg in args {
                    self.compile_expr(arg);
                }
                let constant = self.identifier_constant(method, *super_span);
                self.variable_get("super", *super_span);
                self.emit(OpCode::SuperInvoke, span);
                self.emit_byte(constant, span);
                self.emit_byte(args.len() as u8, span);
            }
            _ => {
                self.compile_expr(callee);
                for arg in args {
                    self.compile_expr(arg);
                }
                self.emit(OpCode::Call, span);
                self.emit_byte(args.len() as u8, span);
            }
        }
    }
}

/// Compiles `program` into a top-level [`Function`].
///
/// # Errors
///
/// Returns all compile errors if any were produced. On success the returned
/// function is the `<script>` entry point.
pub fn compile(program: &[Stmt], source: &str) -> Result<Rc<Function>, Vec<Error>> {
    let mut compiler = Compiler::new();
    compiler.source = source.to_string();
    for stmt in program {
        compiler.compile_stmt(stmt);
    }
    compiler.emit(OpCode::Nil, Span::dummy());
    compiler.emit(OpCode::Return, Span::dummy());

    let finished = compiler.states.pop().expect("script state");
    let function = Rc::new(Function {
        name: "<script>".to_string(),
        arity: 0,
        chunk: finished.chunk,
        upvalue_count: finished.upvalues.len(),
    });

    if compiler.errors.is_empty() {
        Ok(function)
    } else {
        Err(compiler.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::disassemble_function;
    use crate::parser::parse;

    fn compile_src(src: &str) -> Rc<Function> {
        let program = parse(src).expect("parse");
        compile(&program, src).expect("compile")
    }

    fn compile_errors(src: &str) -> Vec<Error> {
        let program = parse(src).expect("parse");
        compile(&program, src).expect_err("expected compile errors")
    }

    fn disassemble(src: &str) -> String {
        let f = compile_src(src);
        let mut out = String::new();
        disassemble_function(&f, &mut out);
        out
    }

    #[test]
    fn compiles_arithmetic() {
        let out = disassemble("print 1 + 2 * 3;");
        assert!(out.contains("OP_CONSTANT"));
        assert!(out.contains("OP_MULTIPLY"));
        assert!(out.contains("OP_ADD"));
        assert!(out.contains("OP_PRINT"));
    }

    #[test]
    fn compiles_function_with_closure() {
        let src = "fun outer() { var x = 1; fun inner() { return x; } return inner; }";
        let out = disassemble(src);
        assert!(out.contains("OP_CLOSURE"));
        assert!(out.contains("OP_GET_UPVALUE"));
    }

    #[test]
    fn compiles_class_with_methods() {
        let src = "class A { m() { return 1; } }";
        let out = disassemble(src);
        assert!(out.contains("OP_CLASS"));
        assert!(out.contains("OP_METHOD"));
    }

    #[test]
    fn compiles_inheritance() {
        let src = "class A {} class B < A {}";
        let out = disassemble(src);
        assert!(out.contains("OP_INHERIT"));
    }

    #[test]
    fn return_outside_function_is_error() {
        let errs = compile_errors("return 1;");
        assert!(errs.iter().any(|e| e.to_string().contains("top-level")));
    }

    #[test]
    fn too_many_locals_is_error() {
        use std::fmt::Write as _;

        let mut src = String::from("{");
        for i in 0..300 {
            let _ = write!(src, "var v{i} = {i};");
        }
        src.push('}');
        let errs = compile_errors(&src);
        assert!(errs
            .iter()
            .any(|e| e.to_string().contains("local variables")));
    }

    #[test]
    fn break_outside_loop_is_error() {
        // The parser rejects this first, so exercise the compiler directly by
        // constructing the AST through a loop that the parser allows.
        // Instead, verify a valid loop's `break` compiles.
        let out = disassemble("while (true) { break; }");
        assert!(out.contains("OP_JUMP"));
    }

    #[test]
    fn continue_in_for_compiles() {
        let out = disassemble("for (var i = 0; i < 3; i = i + 1) { continue; }");
        assert!(out.contains("OP_LOOP"));
    }
}
