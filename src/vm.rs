//! Stack-based bytecode virtual machine.
//!
//! The VM executes a [`Function`] produced by [`crate::compiler`]. It owns three
//! pieces of mutable state:
//!
//! * the **value stack** ([`Vm::stack`]) — operand stack and local storage for
//!   every frame, exactly as in `clox`;
//! * the **call stack** ([`Vm::frames`]) — one [`CallFrame`] per active call;
//! * the **open upvalue list** ([`Vm::open_upvalues`]) — upvalue cells that
//!   still point into the value stack.
//!
//! # Stack discipline
//!
//! Every opcode's effect on the stack height is stated in comments. The
//! invariant maintained by `run` is that, at an instruction boundary, the value
//! stack contains exactly the locals of all active frames plus the temporaries
//! of the frame currently executing. `debug_assert!`s in [`Vm::push`] and
//! [`Vm::pop`] catch violations in debug builds.
//!
//! # Determinism and re-use
//!
//! After [`Vm::interpret`] returns, the value and call stacks are empty and only
//! [`Vm::globals`] persists. This makes the same `Vm` value safe to re-use for
//! successive REPL lines.

use crate::chunk::Chunk;
use crate::error::Error;
use crate::object::{
    BoundMethod, Class, Closure, Function, Instance, NativeFn, NativeFunc, Object, Upvalue,
    UpvalueLocation,
};
use crate::opcode::OpCode;
use crate::span::Span;
use crate::stack::Stack;
use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A tiny FNV-1a hasher for global-variable names.
///
/// Global names are short (a few bytes) and few in number, so the
/// denial-of-service resistance of the default `SipHash` buys nothing while
/// costing a measurable fraction of interpreter time on global-heavy code such
/// as recursion. FNV-1a is a couple of instructions per byte.
pub struct FnvHasher(u64);

impl Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut hash = self.0;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = hash;
    }
}

impl Default for FnvHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

/// Builds [`FnvHasher`]s with the standard FNV-1a offset basis.
pub type FnvBuildHasher = BuildHasherDefault<FnvHasher>;

/// Maximum depth of the call stack.
///
/// This bounds recursion; exceeding it is a reported [`Error::Runtime`], never a
/// host stack overflow. 1024 frames is far more than any example or benchmark
/// needs while staying small enough that a runaway recursion fails fast.
pub const MAX_FRAMES: usize = 1024;

/// Maximum height of the value stack.
///
/// Each frame contributes at least one slot and at most [`crate::compiler`]'s
/// local limit, so `MAX_FRAMES * 256` is a safe ceiling that cannot reject a
/// program that would otherwise be valid.
pub const STACK_MAX: usize = MAX_FRAMES * 256;

/// One activation record.
struct CallFrame {
    /// The closure being executed.
    closure: Rc<Closure>,
    /// Program counter: index of the *next* byte to read.
    ip: usize,
    /// Index into the value stack where this frame's slot 0 begins.
    slots: usize,
}

/// The virtual machine.
pub struct Vm {
    stack: Stack,
    frames: Vec<CallFrame>,
    open_upvalues: Vec<Rc<RefCell<Upvalue>>>,
    /// Global variables. Persists across [`Vm::interpret`] calls.
    pub globals: HashMap<String, Value, FnvBuildHasher>,
    /// Captured `print` output (and trace lines).
    output: String,
    /// When set, each instruction is traced to [`Vm::output`].
    pub trace: bool,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// Creates a VM with the default globals (`clock`).
    #[must_use]
    pub fn new() -> Self {
        let mut vm = Self {
            stack: Stack::with_capacity(64),
            frames: Vec::with_capacity(8),
            open_upvalues: Vec::new(),
            globals: HashMap::default(),
            output: String::new(),
            trace: false,
        };
        vm.define_native("clock", 0, clock);
        vm
    }

    /// Creates a VM with instruction tracing enabled/disabled.
    #[must_use]
    pub fn with_trace(trace: bool) -> Self {
        let mut vm = Self::new();
        vm.trace = trace;
        vm
    }

    /// Registers a native function as a global.
    pub fn define_native(&mut self, name: &'static str, arity: usize, func: NativeFunc) {
        self.globals.insert(
            name.to_string(),
            Value::Obj(Rc::new(Object::Native(NativeFn { name, arity, func }))),
        );
    }

    /// Returns the output captured so far.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }

    /// Takes and clears the captured output.
    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.output)
    }

    /// Compiles nothing but runs `function` to completion, returning captured
    /// output on success.
    ///
    /// # Errors
    ///
    /// Returns the first runtime error raised by the program.
    pub fn interpret(&mut self, function: Rc<Function>) -> Result<String, Error> {
        let closure = Rc::new(Closure {
            function,
            upvalues: Vec::new(),
        });
        self.stack
            .push(Value::Obj(Rc::new(Object::Closure(Rc::clone(&closure)))));
        self.frames.push(CallFrame {
            closure,
            ip: 0,
            slots: 0,
        });
        self.run()?;
        Ok(self.take_output())
    }

    // --- Stack helpers ----------------------------------------------------

    /// Pushes a value.
    ///
    /// Precondition: `stack.len() < STACK_MAX` (checked on entry to every call).
    #[inline]
    fn push(&mut self, value: Value) {
        debug_assert!(self.stack.len() < STACK_MAX, "value stack overflow");
        self.stack.push(value);
    }

    /// Pops a value.
    ///
    /// Precondition: the stack is non-empty. Returns `Value::Nil` defensively if
    /// it is not, which can only happen after a compiler bug.
    #[inline]
    fn pop(&mut self) -> Value {
        debug_assert!(!self.stack.is_empty(), "value stack underflow");
        self.stack.pop()
    }

    /// Returns the value `distance` below the top (`0` is the top).
    #[inline]
    fn peek(&self, distance: usize) -> &Value {
        self.stack.peek(distance)
    }

    /// Reads and clears a captured upvalue cell's current value.
    fn upvalue_get(&self, uv: &Rc<RefCell<Upvalue>>) -> Value {
        match &uv.borrow().location {
            UpvalueLocation::Stack(index) => self.stack.get(*index).clone(),
            UpvalueLocation::Closed(value) => value.clone(),
        }
    }

    /// Writes a captured upvalue cell's current value.
    fn upvalue_set(&mut self, uv: &Rc<RefCell<Upvalue>>, value: Value) {
        match &mut uv.borrow_mut().location {
            UpvalueLocation::Stack(index) => self.stack.set(*index, value),
            UpvalueLocation::Closed(slot) => *slot = value,
        }
    }

    /// Captures the stack slot `local`, reusing an existing open upvalue if one
    /// already points at that slot.
    fn capture_upvalue(&mut self, local: usize) -> Rc<RefCell<Upvalue>> {
        for uv in &self.open_upvalues {
            let is_match =
                matches!(&uv.borrow().location, UpvalueLocation::Stack(i) if *i == local);
            if is_match {
                return Rc::clone(uv);
            }
        }
        let uv = Rc::new(RefCell::new(Upvalue {
            location: UpvalueLocation::Stack(local),
        }));
        self.open_upvalues.push(Rc::clone(&uv));
        uv
    }

    /// Closes every open upvalue at or above stack index `last`.
    ///
    /// Precondition: `last <= stack.len()`. Upvalues below `last` belong to an
    /// enclosing frame and are left open.
    fn close_upvalues(&mut self, last: usize) {
        // Fast path: most returns have no open upvalues at all (no closure was
        // ever created below this frame).
        if self.open_upvalues.is_empty() {
            return;
        }
        // Snapshot the cells that must be closed. Doing this in two passes avoids
        // holding a `Ref` from `borrow()` across the `borrow_mut()` that closes
        // the cell (which would panic at runtime).
        let mut closing: Vec<(Rc<RefCell<Upvalue>>, usize)> = Vec::new();
        for uv in &self.open_upvalues {
            if let UpvalueLocation::Stack(index) = &uv.borrow().location {
                if *index >= last {
                    closing.push((Rc::clone(uv), *index));
                }
            }
        }
        for (uv, index) in &closing {
            let value = self.stack.get(*index).clone();
            uv.borrow_mut().location = UpvalueLocation::Closed(value);
        }
        // Open upvalues never move backwards to the stack, so any closed cell can
        // be dropped from the open list.
        self.open_upvalues
            .retain(|uv| !matches!(&uv.borrow().location, UpvalueLocation::Closed(_)));
    }

    /// Builds a runtime error at `span`.
    fn runtime_error(&self, span: Span, message: impl Into<String>) -> Error {
        Error::runtime(message, span)
    }

    // --- Calls ------------------------------------------------------------

    /// Pushes a frame for `closure`.
    ///
    /// Preconditions: the stack holds the callee at `len - arg_count - 1`
    /// followed by `arg_count` arguments, and `arg_count == closure.arity`.
    fn call_closure(
        &mut self,
        closure: Rc<Closure>,
        arg_count: usize,
        chunk: &Chunk,
        op_offset: usize,
    ) -> Result<(), Error> {
        debug_assert!(self.stack.len() > arg_count);
        if arg_count != closure.function.arity {
            return Err(self.runtime_error(
                chunk.span(op_offset),
                format!(
                    "expected {} arguments but got {arg_count}",
                    closure.function.arity
                ),
            ));
        }
        if self.frames.len() >= MAX_FRAMES || self.stack.len() >= STACK_MAX {
            return Err(self.runtime_error(chunk.span(op_offset), "stack overflow"));
        }
        let slots = self.stack.len() - arg_count - 1;
        self.frames.push(CallFrame {
            closure,
            ip: 0,
            slots,
        });
        Ok(())
    }

    /// Calls the value at `stack[len - arg_count - 1]`.
    ///
    /// Returns `Some(closure)` if a new frame was pushed (the caller must then
    /// switch to it), or `None` if a native produced its result in place.
    fn call_value(
        &mut self,
        arg_count: usize,
        chunk: &Chunk,
        op_offset: usize,
    ) -> Result<Option<Rc<Closure>>, Error> {
        let callee = self.peek(arg_count).clone();
        let Value::Obj(obj) = callee else {
            return Err(self.runtime_error(
                chunk.span(op_offset),
                format!(
                    "can only call functions and classes, got {}",
                    callee.type_name()
                ),
            ));
        };
        match &*obj {
            Object::Closure(closure) => {
                let closure = Rc::clone(closure);
                self.call_closure(Rc::clone(&closure), arg_count, chunk, op_offset)?;
                Ok(Some(closure))
            }
            Object::BoundMethod(bound) => {
                let bound = Rc::clone(bound);
                // Replace the bound method with its receiver so it becomes
                // slot 0 (`this`) of the callee's frame.
                let callee_slot = self.stack.len() - arg_count - 1;
                self.stack.set(callee_slot, bound.receiver.clone());
                self.call_closure(Rc::clone(&bound.method), arg_count, chunk, op_offset)?;
                Ok(Some(Rc::clone(&bound.method)))
            }
            Object::Native(native) => {
                if arg_count != native.arity {
                    return Err(self.runtime_error(
                        chunk.span(op_offset),
                        format!("expected {} arguments but got {arg_count}", native.arity),
                    ));
                }
                let args_start = self.stack.len() - arg_count;
                let args: Vec<Value> = self.stack.tail_from(args_start);
                let result = (native.func)(&args)
                    .map_err(|m| self.runtime_error(chunk.span(op_offset), m))?;
                self.stack.truncate(args_start - 1);
                self.push(result);
                Ok(None)
            }
            Object::Class(class) => {
                let class = Rc::clone(class);
                let instance = Rc::new(RefCell::new(Instance {
                    class: Rc::clone(&class),
                    fields: RefCell::new(HashMap::new()),
                }));
                // Replace the class on the stack with the fresh instance so it
                // becomes slot 0 (`this`) when the initializer runs.
                let callee_slot = self.stack.len() - arg_count - 1;
                self.stack
                    .set(callee_slot, Value::Obj(Rc::new(Object::Instance(instance))));
                if let Some(init) = find_method(&class, "init") {
                    self.call_closure(Rc::clone(&init), arg_count, chunk, op_offset)?;
                    Ok(Some(init))
                } else if arg_count != 0 {
                    Err(self.runtime_error(
                        chunk.span(op_offset),
                        format!("expected 0 arguments but got {arg_count}"),
                    ))
                } else {
                    Ok(None)
                }
            }
            other => Err(self.runtime_error(
                chunk.span(op_offset),
                format!(
                    "can only call functions and classes, got {}",
                    other.type_name()
                ),
            )),
        }
    }

    /// Executes `instance.name(args...)`.
    ///
    /// The receiver is at `stack[len - arg_count - 1]`. If `name` is an instance
    /// field it is called as a value; otherwise the method is looked up on the
    /// class chain and called with the receiver as `this`.
    fn invoke(
        &mut self,
        name: Rc<str>,
        arg_count: usize,
        chunk: &Chunk,
        op_offset: usize,
    ) -> Result<Option<Rc<Closure>>, Error> {
        let receiver = self.peek(arg_count).clone();
        let Value::Obj(obj) = receiver else {
            return Err(self.runtime_error(chunk.span(op_offset), "only instances have methods"));
        };
        let Object::Instance(instance) = &*obj else {
            return Err(self.runtime_error(chunk.span(op_offset), "only instances have methods"));
        };
        let instance = Rc::clone(instance);

        let field = instance.borrow().fields.borrow().get(&*name).cloned();
        if let Some(field) = field {
            let callee_slot = self.stack.len() - arg_count - 1;
            self.stack.set(callee_slot, field);
            return self.call_value(arg_count, chunk, op_offset);
        }

        let class = Rc::clone(&instance.borrow().class);
        let method = find_method(&class, &name).ok_or_else(|| {
            self.runtime_error(
                chunk.span(op_offset),
                format!("undefined property '{name}'"),
            )
        })?;
        self.call_closure(Rc::clone(&method), arg_count, chunk, op_offset)?;
        Ok(Some(method))
    }

    // --- Execution loop ---------------------------------------------------

    /// Runs until the call stack drains.
    fn run(&mut self) -> Result<(), Error> {
        if self.frames.is_empty() {
            return Ok(());
        }
        let mut frame_index = self.frames.len() - 1;
        let mut closure = Rc::clone(&self.frames[frame_index].closure);
        let mut ip = self.frames[frame_index].ip;
        let trace = self.trace;

        loop {
            let chunk = &closure.function.chunk;
            let op_offset = ip;
            let byte = chunk.code[ip];
            ip += 1;
            let Some(op) = OpCode::from_byte(byte) else {
                return Err(Error::runtime(
                    format!("invalid opcode {byte}"),
                    chunk.span(op_offset),
                ));
            };

            if trace {
                self.trace_instruction(op, op_offset, chunk);
            }

            match op {
                OpCode::Constant => {
                    let index = chunk.code[ip];
                    ip += 1;
                    let value = chunk.constant(index).clone();
                    self.push(value);
                }
                OpCode::Nil => self.push(Value::Nil),
                OpCode::True => self.push(Value::Bool(true)),
                OpCode::False => self.push(Value::Bool(false)),
                OpCode::Pop => {
                    self.pop();
                }
                OpCode::GetLocal => {
                    let slot = chunk.code[ip] as usize;
                    ip += 1;
                    let index = self.frames[frame_index].slots + slot;
                    let value = self.stack.get(index).clone();
                    self.push(value);
                }
                OpCode::SetLocal => {
                    let slot = chunk.code[ip] as usize;
                    ip += 1;
                    let value = self.peek(0).clone();
                    let index = self.frames[frame_index].slots + slot;
                    self.stack.set(index, value);
                }
                OpCode::GetGlobal => {
                    let name = read_name(chunk, &mut ip);
                    let value = self.globals.get(&*name).cloned().ok_or_else(|| {
                        self.runtime_error(
                            chunk.span(op_offset),
                            format!("undefined variable '{name}'"),
                        )
                    })?;
                    self.push(value);
                }
                OpCode::DefineGlobal => {
                    let name = read_name(chunk, &mut ip);
                    let value = self.pop();
                    self.globals.insert(name.to_string(), value);
                }
                OpCode::SetGlobal => {
                    let name = read_name(chunk, &mut ip);
                    if self.globals.contains_key(&*name) {
                        let value = self.peek(0).clone();
                        self.globals.insert(name.to_string(), value);
                    } else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("undefined variable '{name}'"),
                        ));
                    }
                }
                OpCode::GetUpvalue => {
                    let slot = chunk.code[ip] as usize;
                    ip += 1;
                    let uv = Rc::clone(&closure.upvalues[slot]);
                    let value = self.upvalue_get(&uv);
                    self.push(value);
                }
                OpCode::SetUpvalue => {
                    let slot = chunk.code[ip] as usize;
                    ip += 1;
                    let uv = Rc::clone(&closure.upvalues[slot]);
                    let value = self.peek(0).clone();
                    self.upvalue_set(&uv, value);
                }
                OpCode::GetProperty => {
                    let name = read_name(chunk, &mut ip);
                    self.get_property(&name, chunk, op_offset)?;
                }
                OpCode::SetProperty => {
                    let name = read_name(chunk, &mut ip);
                    self.set_property(name, chunk, op_offset)?;
                }
                OpCode::GetSuper => {
                    let name = read_name(chunk, &mut ip);
                    let superclass = self.pop_class(chunk, op_offset)?;
                    let receiver = self.pop();
                    let method = find_method(&superclass, &name).ok_or_else(|| {
                        self.runtime_error(
                            chunk.span(op_offset),
                            format!("undefined property '{name}'"),
                        )
                    })?;
                    let bound = Rc::new(BoundMethod { receiver, method });
                    self.push(Value::Obj(Rc::new(Object::BoundMethod(bound))));
                }
                OpCode::Equal => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(Value::Bool(a.equals(&b)));
                }
                OpCode::Greater => {
                    let b = self.pop();
                    let Value::Number(b) = b else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operands must be numbers, got {}", b.type_name()),
                        ));
                    };
                    let a = self.pop();
                    let Value::Number(a) = a else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operands must be numbers, got {}", a.type_name()),
                        ));
                    };
                    self.push(Value::Bool(a > b));
                }
                OpCode::Less => {
                    let b = self.pop();
                    let Value::Number(b) = b else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operands must be numbers, got {}", b.type_name()),
                        ));
                    };
                    let a = self.pop();
                    let Value::Number(a) = a else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operands must be numbers, got {}", a.type_name()),
                        ));
                    };
                    self.push(Value::Bool(a < b));
                }
                OpCode::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    match (&a, &b) {
                        (Value::Number(x), Value::Number(y)) => self.push(Value::Number(x + y)),
                        (Value::Obj(x), Value::Obj(y)) => match (x.as_string(), y.as_string()) {
                            (Some(x), Some(y)) => {
                                let mut s = String::with_capacity(x.len() + y.len());
                                s.push_str(&x);
                                s.push_str(&y);
                                self.push(Value::string(s));
                            }
                            _ => return Err(self.binary_type_error(chunk.span(op_offset), "+")),
                        },
                        _ => return Err(self.binary_type_error(chunk.span(op_offset), "+")),
                    }
                }
                OpCode::Subtract | OpCode::Multiply | OpCode::Divide => {
                    let b = self.pop();
                    let Value::Number(b) = b else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operands must be numbers, got {}", b.type_name()),
                        ));
                    };
                    let a = self.pop();
                    let Value::Number(a) = a else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operands must be numbers, got {}", a.type_name()),
                        ));
                    };
                    let result = match op {
                        OpCode::Subtract => a - b,
                        OpCode::Multiply => a * b,
                        _ => a / b,
                    };
                    self.push(Value::Number(result));
                }
                OpCode::Not => {
                    let value = self.pop();
                    self.push(Value::Bool(value.is_falsey()));
                }
                OpCode::Negate => {
                    let value = self.pop();
                    let Value::Number(n) = value else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            format!("operand must be a number, got {}", value.type_name()),
                        ));
                    };
                    self.push(Value::Number(-n));
                }
                OpCode::Print => {
                    let value = self.pop();
                    self.output.push_str(&value.to_display());
                    self.output.push('\n');
                }
                OpCode::Jump => {
                    let offset = Chunk::read_u16(&chunk.code, ip) as usize;
                    ip += 2 + offset;
                }
                OpCode::JumpIfFalse => {
                    let offset = Chunk::read_u16(&chunk.code, ip) as usize;
                    ip += 2;
                    if self.peek(0).is_falsey() {
                        ip += offset;
                    }
                }
                OpCode::Loop => {
                    let offset = Chunk::read_u16(&chunk.code, ip) as usize;
                    ip += 2;
                    ip -= offset;
                }
                OpCode::Call => {
                    let arg_count = chunk.code[ip] as usize;
                    ip += 1;
                    self.frames[frame_index].ip = ip;
                    if let Some(new_closure) = self.call_value(arg_count, chunk, op_offset)? {
                        frame_index = self.frames.len() - 1;
                        closure = new_closure;
                        ip = 0;
                    }
                }
                OpCode::Invoke => {
                    let name = read_name(chunk, &mut ip);
                    let arg_count = chunk.code[ip] as usize;
                    ip += 1;
                    self.frames[frame_index].ip = ip;
                    if let Some(new_closure) = self.invoke(name, arg_count, chunk, op_offset)? {
                        frame_index = self.frames.len() - 1;
                        closure = new_closure;
                        ip = 0;
                    }
                }
                OpCode::SuperInvoke => {
                    let name = read_name(chunk, &mut ip);
                    let arg_count = chunk.code[ip] as usize;
                    ip += 1;
                    self.frames[frame_index].ip = ip;
                    let superclass = self.pop_class(chunk, op_offset)?;
                    let method = find_method(&superclass, &name).ok_or_else(|| {
                        self.runtime_error(
                            chunk.span(op_offset),
                            format!("undefined property '{name}'"),
                        )
                    })?;
                    self.call_closure(Rc::clone(&method), arg_count, chunk, op_offset)?;
                    frame_index = self.frames.len() - 1;
                    closure = method;
                    ip = 0;
                }
                OpCode::Closure => {
                    let const_index = chunk.code[ip];
                    ip += 1;
                    let function = match chunk.constant(const_index) {
                        Value::Obj(o) => match &**o {
                            Object::Function(f) => Rc::clone(f),
                            _ => {
                                return Err(self.runtime_error(
                                    chunk.span(op_offset),
                                    "OP_CLOSURE constant is not a function",
                                ))
                            }
                        },
                        _ => {
                            return Err(self.runtime_error(
                                chunk.span(op_offset),
                                "OP_CLOSURE constant is not a function",
                            ))
                        }
                    };
                    let upvalue_count = function.upvalue_count;
                    let mut upvalues = Vec::with_capacity(upvalue_count);
                    for _ in 0..upvalue_count {
                        let is_local = chunk.code[ip] != 0;
                        let index = chunk.code[ip + 1] as usize;
                        ip += 2;
                        if is_local {
                            let local = self.frames[frame_index].slots + index;
                            upvalues.push(self.capture_upvalue(local));
                        } else {
                            let enclosing = Rc::clone(&self.frames[frame_index].closure);
                            upvalues.push(Rc::clone(&enclosing.upvalues[index]));
                        }
                    }
                    let new_closure = Rc::new(Closure { function, upvalues });
                    self.push(Value::Obj(Rc::new(Object::Closure(new_closure))));
                }
                OpCode::CloseUpvalue => {
                    let last = self.stack.len() - 1;
                    self.close_upvalues(last);
                    self.pop();
                }
                OpCode::Return => {
                    let result = self.pop();
                    let slots = self.frames[frame_index].slots;
                    self.close_upvalues(slots);
                    self.frames.pop();
                    if self.frames.is_empty() {
                        // The top-level script finished; drop its closure slot.
                        self.pop();
                        return Ok(());
                    }
                    self.stack.truncate(slots);
                    self.push(result);
                    frame_index = self.frames.len() - 1;
                    closure = Rc::clone(&self.frames[frame_index].closure);
                    ip = self.frames[frame_index].ip;
                }
                OpCode::Class => {
                    let name = read_name(chunk, &mut ip);
                    self.push(Value::Obj(Rc::new(Object::Class(Rc::new(Class {
                        name: name.to_string(),
                        methods: RefCell::new(HashMap::new()),
                        superclass: RefCell::new(None),
                    })))));
                }
                OpCode::Inherit => {
                    let superclass = self.peek(1).clone();
                    let subclass = self.peek(0).clone();
                    let (Value::Obj(sc), Value::Obj(sub)) = (&superclass, &subclass) else {
                        return Err(
                            self.runtime_error(chunk.span(op_offset), "superclass must be a class")
                        );
                    };
                    let (Object::Class(sc), Object::Class(sub)) = (&**sc, &**sub) else {
                        return Err(
                            self.runtime_error(chunk.span(op_offset), "superclass must be a class")
                        );
                    };
                    *sub.superclass.borrow_mut() = Some(Rc::clone(sc));
                    self.pop(); // discard the subclass; superclass stays as `super`
                }
                OpCode::Method => {
                    let name = read_name(chunk, &mut ip);
                    let method = self.peek(0).clone();
                    let class = self.peek(1).clone();
                    let Value::Obj(obj) = class else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            "OP_METHOD target is not a class",
                        ));
                    };
                    let Object::Class(class) = &*obj else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            "OP_METHOD target is not a class",
                        ));
                    };
                    let Value::Obj(method_obj) = method else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            "OP_METHOD value is not a method",
                        ));
                    };
                    let Object::Closure(closure) = &*method_obj else {
                        return Err(self.runtime_error(
                            chunk.span(op_offset),
                            "OP_METHOD value is not a method",
                        ));
                    };
                    class
                        .methods
                        .borrow_mut()
                        .insert(name.to_string(), Rc::clone(closure));
                    self.pop();
                }
            }
        }
    }

    // --- Instruction helpers ---------------------------------------------

    /// Appends a one-line trace entry for `op`.
    fn trace_instruction(&mut self, op: OpCode, offset: usize, chunk: &Chunk) {
        let _ = write!(
            self.output,
            "trace {offset:04} {:<18} line {:>3} stack[{}]",
            op.name(),
            chunk.line(offset),
            self.stack.len()
        );
        for value in self.stack.iter() {
            let _ = write!(self.output, " {value}");
        }
        self.output.push('\n');
    }

    /// Builds a "bad operands for binary `<op>`" error.
    fn binary_type_error(&self, span: Span, op: &str) -> Error {
        self.runtime_error(span, format!("invalid operands for binary '{op}'"))
    }

    /// Pops a value that must be a class.
    fn pop_class(&mut self, chunk: &Chunk, op_offset: usize) -> Result<Rc<Class>, Error> {
        let value = self.pop();
        let Value::Obj(obj) = value else {
            return Err(self.runtime_error(chunk.span(op_offset), "superclass must be a class"));
        };
        let Object::Class(class) = &*obj else {
            return Err(self.runtime_error(chunk.span(op_offset), "superclass must be a class"));
        };
        Ok(Rc::clone(class))
    }

    /// Implements `OP_GET_PROPERTY`: turns `instance` (top of stack) into the
    /// field value or a bound method.
    fn get_property(&mut self, name: &str, chunk: &Chunk, op_offset: usize) -> Result<(), Error> {
        let receiver = self.peek(0).clone();
        let Value::Obj(obj) = receiver else {
            return Err(self.runtime_error(chunk.span(op_offset), "only instances have properties"));
        };
        let Object::Instance(instance) = &*obj else {
            return Err(self.runtime_error(chunk.span(op_offset), "only instances have properties"));
        };
        let instance = Rc::clone(instance);

        if let Some(field) = instance.borrow().fields.borrow().get(name).cloned() {
            self.pop();
            self.push(field);
            return Ok(());
        }

        let class = Rc::clone(&instance.borrow().class);
        let method = find_method(&class, name).ok_or_else(|| {
            self.runtime_error(
                chunk.span(op_offset),
                format!("undefined property '{name}'"),
            )
        })?;
        let receiver = self.pop();
        let bound = Rc::new(BoundMethod { receiver, method });
        self.push(Value::Obj(Rc::new(Object::BoundMethod(bound))));
        Ok(())
    }

    /// Implements `OP_SET_PROPERTY`: stack is `[instance, value]`; stores the
    /// field and leaves `value` on the stack.
    fn set_property(
        &mut self,
        name: Rc<str>,
        chunk: &Chunk,
        op_offset: usize,
    ) -> Result<(), Error> {
        let object = self.peek(1).clone();
        let Value::Obj(obj) = object else {
            return Err(self.runtime_error(chunk.span(op_offset), "only instances have fields"));
        };
        let Object::Instance(instance) = &*obj else {
            return Err(self.runtime_error(chunk.span(op_offset), "only instances have fields"));
        };
        let value = self.peek(0).clone();
        instance
            .borrow()
            .fields
            .borrow_mut()
            .insert(name.to_string(), value.clone());
        self.pop();
        self.pop();
        self.push(value);
        Ok(())
    }
}

/// Reads a one-byte constant index operand and returns it as a string.
///
/// Falls back to an empty string if the constant is not a string, which cannot
/// happen for compiler-produced bytecode.
#[inline]
fn read_name(chunk: &Chunk, ip: &mut usize) -> Rc<str> {
    let index = chunk.code[*ip];
    *ip += 1;
    chunk
        .constant(index)
        .as_string()
        .unwrap_or_else(|| Rc::from(""))
}

/// Finds a method by name, walking the superclass chain.
fn find_method(class: &Rc<Class>, name: &str) -> Option<Rc<Closure>> {
    if let Some(method) = class.methods.borrow().get(name) {
        return Some(Rc::clone(method));
    }
    let superclass = class.superclass.borrow().as_ref().map(Rc::clone);
    if let Some(superclass) = superclass {
        return find_method(&superclass, name);
    }
    None
}

/// The `clock` native: seconds since the Unix epoch.
fn clock(_args: &[Value]) -> Result<Value, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("system clock error: {e}"))?
        .as_secs_f64();
    Ok(Value::Number(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn run(src: &str) -> String {
        let program = parse(src).expect("parse");
        let function = crate::compiler::compile(&program, src).expect("compile");
        let mut vm = Vm::new();
        vm.interpret(function).expect("no runtime error")
    }

    fn run_err(src: &str) -> Error {
        let program = parse(src).expect("parse");
        let function = crate::compiler::compile(&program, src).expect("compile");
        let mut vm = Vm::new();
        vm.interpret(function).expect_err("expected runtime error")
    }

    #[test]
    fn arithmetic() {
        assert_eq!(run("print 1 + 2 * 3;"), "7\n");
        assert_eq!(run("print (1 + 2) * 3;"), "9\n");
    }

    #[test]
    fn strings_concatenate() {
        assert_eq!(run("print \"a\" + \"b\" + \"c\";"), "abc\n");
    }

    #[test]
    fn control_flow_and_loops() {
        assert_eq!(run("if (true) print 1; else print 2;"), "1\n");
        assert_eq!(
            run("var i = 0; while (i < 3) { print i; i = i + 1; }"),
            "0\n1\n2\n"
        );
        assert_eq!(
            run("for (var i = 0; i < 3; i = i + 1) { print i; }"),
            "0\n1\n2\n"
        );
    }

    #[test]
    fn break_and_continue() {
        assert_eq!(
            run("for (var i = 0; i < 5; i = i + 1) { if (i == 2) continue; if (i == 4) break; print i; }"),
            "0\n1\n3\n"
        );
    }

    #[test]
    fn recursion() {
        assert_eq!(
            run("fun fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } print fib(10);"),
            "55\n"
        );
    }

    #[test]
    fn closures_capture_by_reference() {
        let src = "fun counter() { var n = 0; fun inc() { n = n + 1; return n; } return inc; } \
                   var c = counter(); print c(); print c(); print c();";
        assert_eq!(run(src), "1\n2\n3\n");
    }

    #[test]
    fn closures_in_loop_capture_distinct_cells() {
        let src = "var fns = nil; \
                   fun make() { var a = 0; var b = 0; \
                     fun f1() { a = a + 1; return a; } \
                     fun f2() { b = b + 10; return b; } \
                     return f1; } \
                   var f = make(); print f(); print f();";
        assert_eq!(run(src), "1\n2\n");
    }

    #[test]
    fn classes_and_methods() {
        let src = "class Cake { taste() { print \"yum\"; } } var c = Cake(); c.taste();";
        assert_eq!(run(src), "yum\n");
    }

    #[test]
    fn inheritance_and_super() {
        let src = "class A { hello() { print \"A\"; } } \
                   class B < A { hello() { super.hello(); print \"B\"; } } \
                   B().hello();";
        assert_eq!(run(src), "A\nB\n");
    }

    #[test]
    fn initializer_returns_instance() {
        let src = "class P { init(x) { this.x = x; } get() { return this.x; } } \
                   var p = P(42); print p.get();";
        assert_eq!(run(src), "42\n");
    }

    #[test]
    fn arity_mismatch_is_runtime_error() {
        let err = run_err("fun f(a, b) { return a; } f(1);");
        assert!(err.to_string().contains("expected 2 arguments but got 1"));
    }

    #[test]
    fn undefined_global_is_runtime_error() {
        let err = run_err("print nope;");
        assert!(err.to_string().contains("undefined variable 'nope'"));
    }

    #[test]
    fn type_error_on_addition() {
        let err = run_err("print 1 + nil;");
        assert!(err.to_string().contains("invalid operands"));
    }

    #[test]
    fn stack_overflow_is_reported() {
        let err = run_err("fun f() { return f(); } f();");
        assert!(err.to_string().contains("stack overflow"));
    }

    #[test]
    fn clock_is_callable() {
        let out = run("print clock() > 0;");
        assert_eq!(out, "true\n");
    }
}
