//! Heap-allocated objects referenced by [`crate::value::Value`].
//!
//! # Memory management
//!
//! Vesper uses **reference counting** ([`Rc`]) rather than a tracing garbage
//! collector. This is a deliberate trade-off for this project:
//!
//! * It is completely safe (no `unsafe`, no manual roots), matching the
//!   high-assurance mandate.
//! * Reclamation is prompt and deterministic, which simplifies reasoning about
//!   the VM's object graph.
//! * Mutability of shared objects (class method tables, instance fields,
//!   upvalue cells) is provided by [`RefCell`], checked at runtime.
//!
//! The cost is that reference cycles are not collected. Vesper programs can
//! create cycles only through closures capturing themselves or through
//! instance fields, which is acceptable for the intended educational scope.
//! [`crate::DESIGN.md`] records this as a known limitation and a future work
//! item (a mark-and-sweep collector would replace `Rc`).

use crate::chunk::Chunk;
use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// A compiled function: bytecode plus its constant pool and metadata.
///
/// The [`Chunk`] holds the bytecode; `arity` and `upvalue_count` are needed by
/// the VM to validate calls and to allocate closure cells.
#[derive(Debug)]
pub struct Function {
    /// The function's name (`"<script>"` for the top-level program).
    pub name: String,
    /// Number of parameters (fixed arity; there are no variadic functions).
    pub arity: usize,
    /// The compiled bytecode and constants.
    pub chunk: Chunk,
    /// Number of upvalues captured by this function (matches the descriptors
    /// emitted by `OP_CLOSURE` at each use site).
    pub upvalue_count: usize,
}

/// A runtime closure: a function together with the cells it has captured.
#[derive(Debug)]
pub struct Closure {
    /// The compiled function being executed.
    pub function: Rc<Function>,
    /// Captured upvalue cells, in the order declared by the compiler.
    pub upvalues: Vec<Rc<RefCell<Upvalue>>>,
}

/// A captured variable cell.
#[derive(Debug)]
pub struct Upvalue {
    /// Where the captured value currently lives.
    pub location: UpvalueLocation,
}

/// Location of an upvalue's storage.
#[derive(Debug)]
pub enum UpvalueLocation {
    /// Still on the VM stack at the given absolute stack index (an *open*
    /// upvalue).
    Stack(usize),
    /// Moved off the stack because its defining scope ended (a *closed*
    /// upvalue).
    Closed(Value),
}

/// The signature of a Rust function exposed to Vesper code.
///
/// The function receives the evaluated arguments and returns either a value or
/// a human-readable error message (which the VM wraps into a runtime error at
/// the call site).
pub type NativeFunc = fn(&[Value]) -> Result<Value, String>;

/// A native (Rust-implemented) function.
#[derive(Clone, Copy)]
pub struct NativeFn {
    /// The name shown in diagnostics and `<native fn>` displays.
    pub name: &'static str,
    /// Expected arity.
    pub arity: usize,
    /// The implementation.
    pub func: NativeFunc,
}

/// A class object: a name, a superclass link, and a method table.
pub struct Class {
    /// The class name.
    pub name: String,
    /// Method name -> closure. Methods are stored as closures so they can
    /// capture upvalues from the enclosing scope.
    pub methods: RefCell<HashMap<String, Rc<Closure>>>,
    /// The optional superclass. A `RefCell` is used because `OP_INHERIT` sets
    /// this after the class object has been created.
    pub superclass: RefCell<Option<Rc<Class>>>,
}

/// A class instance: a class link plus mutable fields.
pub struct Instance {
    /// The instance's class.
    pub class: Rc<Class>,
    /// The instance's fields.
    pub fields: RefCell<HashMap<String, Value>>,
}

/// A method bound to its receiver (`instance.method`).
#[derive(Clone)]
pub struct BoundMethod {
    /// The receiver value.
    pub receiver: Value,
    /// The method closure.
    pub method: Rc<Closure>,
}

/// A heap object.
#[derive(Clone)]
pub enum Object {
    /// An immutable string.
    String(Rc<str>),
    /// A compiled function (not yet a closure).
    Function(Rc<Function>),
    /// A closure.
    Closure(Rc<Closure>),
    /// A native function.
    Native(NativeFn),
    /// An upvalue cell (only reachable through a closure).
    Upvalue(Rc<RefCell<Upvalue>>),
    /// A class.
    Class(Rc<Class>),
    /// A class instance.
    Instance(Rc<RefCell<Instance>>),
    /// A bound method.
    BoundMethod(Rc<BoundMethod>),
}

impl Object {
    /// Constructs a string object.
    #[must_use]
    pub fn string(s: impl AsRef<str>) -> Self {
        Self::String(Rc::from(s.as_ref()))
    }

    /// A short, stable type name for diagnostics.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Function(_) => "function",
            Self::Closure(_) => "function",
            Self::Native(_) => "native function",
            Self::Upvalue(_) => "upvalue",
            Self::Class(_) => "class",
            Self::Instance(_) => "instance",
            Self::BoundMethod(_) => "method",
        }
    }

    /// If this object is a string, returns it.
    #[must_use]
    pub fn as_string(&self) -> Option<Rc<str>> {
        match self {
            Self::String(s) => Some(Rc::clone(s)),
            _ => None,
        }
    }

    /// Structural equality for the object kinds where that is meaningful
    /// (strings). All other objects compare by identity.
    #[must_use]
    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Function(a), Self::Function(b)) => Rc::ptr_eq(a, b),
            (Self::Closure(a), Self::Closure(b)) => Rc::ptr_eq(a, b),
            (Self::Native(a), Self::Native(b)) => {
                a.name == b.name
                    && a.arity == b.arity
                    && std::ptr::eq(a.func as *const (), b.func as *const ())
            }
            (Self::Upvalue(a), Self::Upvalue(b)) => Rc::ptr_eq(a, b),
            (Self::Class(a), Self::Class(b)) => Rc::ptr_eq(a, b),
            (Self::Instance(a), Self::Instance(b)) => Rc::ptr_eq(a, b),
            (Self::BoundMethod(a), Self::BoundMethod(b)) => {
                Rc::ptr_eq(&a.method, &b.method) && a.receiver == b.receiver
            }
            _ => false,
        }
    }

    /// Renders the object for user-facing `print` output.
    pub fn display(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => f.write_str(s),
            Self::Function(fun) => write!(f, "<fn {}>", fun.name),
            Self::Closure(c) => write!(f, "<fn {}>", c.function.name),
            Self::Native(n) => write!(f, "<native fn {}>", n.name),
            Self::Upvalue(_) => f.write_str("<upvalue>"),
            Self::Class(c) => write!(f, "{}", c.name),
            Self::Instance(i) => write!(f, "{} instance", i.borrow().class.name),
            Self::BoundMethod(m) => write!(f, "<fn {}>", m.method.function.name),
        }
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Object::{}", self.type_name())
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_objects_compare_by_content() {
        let a = Object::string("hi");
        let b = Object::string("hi");
        let c = Object::string("bye");
        assert!(a.equals(&b));
        assert!(!a.equals(&c));
    }

    #[test]
    fn native_functions_compare_by_identity_and_signature() {
        // The `Result` return is required by the `NativeFunc` signature even
        // though this stub never fails.
        #[allow(clippy::unnecessary_wraps)]
        fn f(_: &[Value]) -> Result<Value, String> {
            Ok(Value::Nil)
        }
        let a = Object::Native(NativeFn {
            name: "clock",
            arity: 0,
            func: f,
        });
        let b = a.clone();
        assert!(a.equals(&b));
    }

    #[test]
    fn type_names() {
        assert_eq!(Object::string("x").type_name(), "string");
    }
}
