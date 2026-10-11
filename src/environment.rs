//! The tree-walking interpreter (Phase 1 baseline) and its runtime environment.
//!
//! This interpreter evaluates the AST directly. It exists for two reasons:
//!
//! 1. **Baseline correctness.** Phase 1 of the project is a working interpreter
//!    independent of the bytecode pipeline.
//! 2. **Test oracle.** The integration tests cross-check the bytecode VM against
//!    this interpreter for programs both can run.
//!
//! It uses the lexical distances computed by [`crate::resolver`] to find local
//! variables, so its scoping matches the bytecode VM exactly.
//!
//! # Interpreter types
//!
//! The tree-walker intentionally defines its **own** value and object types
//! (`TwValue`, `TwFunction`, ...) rather than reusing [`crate::value::Value`] and
//! [`crate::object::Object`]. The VM's objects are bytecode-shaped (they contain
//! [`crate::chunk::Chunk`]s); tree-walker functions are AST-shaped. Keeping them
//! separate avoids polluting the production object model with an interpretation
//! strategy that is not shipped. This is an intentional, documented trade-off.

use crate::ast::{BinaryOp, Expr, LogicalOp, Stmt, UnaryOp};
use crate::error::Error;
use crate::resolver::ResolutionMap;
use crate::span::Span;
use crate::value::format_number;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A lexical environment: a scope of bindings plus an optional parent.
#[derive(Clone, Debug)]
pub struct Environment(Rc<RefCell<EnvInner>>);

#[derive(Debug)]
struct EnvInner {
    values: HashMap<String, TwValue>,
    enclosing: Option<Environment>,
}

impl Environment {
    /// Creates an environment with an optional enclosing scope.
    #[must_use]
    pub fn new(enclosing: Option<Environment>) -> Self {
        Self(Rc::new(RefCell::new(EnvInner {
            values: HashMap::new(),
            enclosing,
        })))
    }

    /// Defines (or overwrites) a binding in this scope.
    pub fn define(&self, name: impl Into<String>, value: TwValue) {
        self.0.borrow_mut().values.insert(name.into(), value);
    }

    /// Looks up a binding in this scope only.
    #[must_use]
    pub fn get_local(&self, name: &str) -> Option<TwValue> {
        self.0.borrow().values.get(name).cloned()
    }

    /// Looks up a binding, searching enclosing scopes.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<TwValue> {
        if let Some(v) = self.get_local(name) {
            return Some(v);
        }
        let enclosing = self.0.borrow().enclosing.clone();
        enclosing.as_ref().and_then(|e| e.get(name))
    }

    /// Assigns to an existing binding, searching enclosing scopes. Returns
    /// `false` if no binding exists.
    pub fn assign(&self, name: &str, value: TwValue) -> bool {
        {
            let mut inner = self.0.borrow_mut();
            if let Some(slot) = inner.values.get_mut(name) {
                *slot = value;
                return true;
            }
        }
        let enclosing = self.0.borrow().enclosing.clone();
        match enclosing {
            Some(e) => e.assign(name, value),
            None => false,
        }
    }

    /// Returns the ancestor `distance` scopes up (0 returns a clone of `self`).
    ///
    /// # Panics
    ///
    /// Panics if `distance` exceeds the scope depth, which indicates a
    /// resolver/interpreter disagreement (a bug, not user error).
    #[must_use]
    pub fn ancestor(&self, distance: usize) -> Environment {
        let mut env = self.clone();
        for _ in 0..distance {
            let parent = env.0.borrow().enclosing.clone();
            env = parent.expect("scope distance exceeds environment depth");
        }
        env
    }
}

/// A value understood by the tree-walking interpreter.
#[derive(Clone, Debug)]
pub enum TwValue {
    /// `nil`.
    Nil,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string.
    Str(Rc<str>),
    /// A user-defined function or method.
    Function(Rc<TwFunction>),
    /// A native function (`name`, `arity`).
    Native(&'static str, usize),
    /// A class.
    Class(Rc<TwClass>),
    /// A class instance.
    Instance(Rc<RefCell<TwInstance>>),
    /// A method bound to a receiver.
    Bound(Rc<TwBound>),
}

impl TwValue {
    fn is_falsey(&self) -> bool {
        matches!(self, Self::Nil | Self::Bool(false))
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "bool",
            Self::Number(_) => "number",
            Self::Str(_) => "string",
            Self::Function(_) => "function",
            Self::Native(..) => "native function",
            Self::Class(_) => "class",
            Self::Instance(_) => "instance",
            Self::Bound(_) => "method",
        }
    }

    fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Instance(a), Self::Instance(b)) => Rc::ptr_eq(a, b),
            (Self::Class(a), Self::Class(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Nil => "nil".to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => format_number(*n),
            Self::Str(s) => s.to_string(),
            Self::Function(f) => format!("<fn {}>", f.name),
            Self::Native(n, _) => format!("<native fn {n}>"),
            Self::Class(c) => c.name.clone(),
            Self::Instance(i) => format!("{} instance", i.borrow().class.name),
            Self::Bound(b) => format!("<fn {}>", b.method.name),
        }
    }
}

/// A tree-walker function: parameters, AST body, and captured environment.
#[derive(Debug)]
pub struct TwFunction {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
    closure: Environment,
    is_initializer: bool,
}

/// A tree-walker class.
#[derive(Debug)]
pub struct TwClass {
    name: String,
    superclass: Option<Rc<TwClass>>,
    methods: HashMap<String, Rc<TwFunction>>,
}

/// A tree-walker instance.
#[derive(Debug)]
pub struct TwInstance {
    class: Rc<TwClass>,
    fields: HashMap<String, TwValue>,
}

/// A method bound to a receiver.
#[derive(Debug)]
pub struct TwBound {
    receiver: TwValue,
    method: Rc<TwFunction>,
}

/// Control-flow signal produced by executing a statement.
enum Flow {
    Normal,
    Return(TwValue),
    Break,
    Continue,
}

/// The tree-walking interpreter.
pub struct TreeWalker {
    globals: Environment,
    env: Environment,
    resolutions: ResolutionMap,
    output: String,
}

impl TreeWalker {
    /// Creates a walker with fresh globals and the given resolutions.
    #[must_use]
    pub fn new(resolutions: ResolutionMap) -> Self {
        let globals = Environment::new(None);
        globals.define("clock", TwValue::Native("clock", 0));
        Self {
            env: globals.clone(),
            globals,
            resolutions,
            output: String::new(),
        }
    }

    /// Returns captured `print` output.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }

    fn runtime_error(&self, span: Span, message: impl Into<String>) -> Error {
        Error::runtime(message, span)
    }

    /// Runs the whole program, returning captured output on success.
    ///
    /// # Errors
    ///
    /// Propagates the first runtime error encountered.
    pub fn interpret(&mut self, program: &[Stmt]) -> Result<(), Error> {
        for stmt in program {
            match self.exec(stmt)? {
                Flow::Normal => {}
                Flow::Return(_) => {
                    return Err(
                        self.runtime_error(stmt.span(), "cannot return from top-level code")
                    );
                }
                Flow::Break | Flow::Continue => {
                    return Err(self.runtime_error(stmt.span(), "loop control outside of a loop"));
                }
            }
        }
        Ok(())
    }

    fn exec(&mut self, stmt: &Stmt) -> Result<Flow, Error> {
        match stmt {
            Stmt::Expression { expr, .. } => {
                self.eval(expr)?;
                Ok(Flow::Normal)
            }
            Stmt::Print { expr, .. } => {
                let value = self.eval(expr)?;
                self.output.push_str(&value.display());
                self.output.push('\n');
                Ok(Flow::Normal)
            }
            Stmt::Var {
                name, initializer, ..
            } => {
                let value = match initializer {
                    Some(e) => self.eval(e)?,
                    None => TwValue::Nil,
                };
                self.env.define(name.clone(), value);
                Ok(Flow::Normal)
            }
            Stmt::Block { statements, .. } => {
                let previous = self.env.clone();
                self.env = Environment::new(Some(previous.clone()));
                let result = (|| {
                    for s in statements {
                        match self.exec(s)? {
                            Flow::Normal => {}
                            other => return Ok(other),
                        }
                    }
                    Ok(Flow::Normal)
                })();
                self.env = previous;
                result
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                if !self.eval(condition)?.is_falsey() {
                    self.exec(then_branch)
                } else if let Some(else_b) = else_branch {
                    self.exec(else_b)
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::While {
                condition, body, ..
            } => {
                loop {
                    if self.eval(condition)?.is_falsey() {
                        break;
                    }
                    match self.exec(body)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        ret @ Flow::Return(_) => return Ok(ret),
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::For {
                init,
                condition,
                increment,
                body,
                ..
            } => {
                // The `for` header introduces its own scope for `init`.
                let previous = self.env.clone();
                self.env = Environment::new(Some(previous.clone()));
                let result = (|| {
                    if let Some(init_stmt) = init {
                        if let Flow::Return(v) = self.exec(init_stmt)? {
                            return Ok(Flow::Return(v));
                        }
                    }
                    loop {
                        if let Some(cond) = condition {
                            if self.eval(cond)?.is_falsey() {
                                break;
                            }
                        }
                        match self.exec(body)? {
                            Flow::Normal | Flow::Continue => {}
                            Flow::Break => break,
                            ret @ Flow::Return(_) => return Ok(ret),
                        }
                        if let Some(inc) = increment {
                            self.eval(inc)?;
                        }
                    }
                    Ok(Flow::Normal)
                })();
                self.env = previous;
                result
            }
            Stmt::Function { name, .. } => {
                let function = self.make_function(name, stmt, &self.env.clone());
                self.env.define(name.clone(), TwValue::Function(function));
                Ok(Flow::Normal)
            }
            Stmt::Return { value, .. } => {
                let value = match value {
                    Some(e) => self.eval(e)?,
                    None => TwValue::Nil,
                };
                Ok(Flow::Return(value))
            }
            Stmt::Break { .. } => Ok(Flow::Break),
            Stmt::Continue { .. } => Ok(Flow::Continue),
            Stmt::Class {
                name,
                superclass,
                methods,
                ..
            } => {
                let superclass = match superclass {
                    Some(e) => match self.eval(e)? {
                        TwValue::Class(c) => Some(c),
                        other => {
                            return Err(self.runtime_error(
                                e.span(),
                                format!("superclass must be a class, got {}", other.type_name()),
                            ))
                        }
                    },
                    None => None,
                };

                // Methods close over an environment that also binds `super`.
                let method_closure = if let Some(sc) = &superclass {
                    let env = Environment::new(Some(self.env.clone()));
                    env.define("super", TwValue::Class(Rc::clone(sc)));
                    env
                } else {
                    self.env.clone()
                };

                let mut table = HashMap::new();
                for method in methods {
                    if let Stmt::Function { name: mname, .. } = method {
                        let f = self.make_function(mname, method, &method_closure);
                        table.insert(mname.clone(), f);
                    }
                }
                let class = Rc::new(TwClass {
                    name: name.clone(),
                    superclass,
                    methods: table,
                });
                self.env.define(name.clone(), TwValue::Class(class));
                Ok(Flow::Normal)
            }
        }
    }

    fn make_function(&self, name: &str, stmt: &Stmt, closure: &Environment) -> Rc<TwFunction> {
        if let Stmt::Function {
            params,
            body,
            name: n,
            ..
        } = stmt
        {
            Rc::new(TwFunction {
                name: n.clone(),
                params: params.clone(),
                body: body.clone(),
                closure: closure.clone(),
                is_initializer: name == "init",
            })
        } else {
            Rc::new(TwFunction {
                name: name.to_string(),
                params: Vec::new(),
                body: Vec::new(),
                closure: closure.clone(),
                is_initializer: false,
            })
        }
    }

    fn eval(&mut self, expr: &Expr) -> Result<TwValue, Error> {
        match expr {
            Expr::Literal { value, .. } => Ok(match value {
                crate::token::Literal::Number(n) => TwValue::Number(*n),
                crate::token::Literal::String(s) => TwValue::Str(Rc::from(s.as_str())),
                crate::token::Literal::Bool(b) => TwValue::Bool(*b),
                crate::token::Literal::Nil => TwValue::Nil,
            }),
            Expr::Grouping { expression, .. } => self.eval(expression),
            Expr::Variable { name, span } => {
                let value = if let Some(distance) = self.resolutions.get(*span) {
                    self.env.ancestor(distance).get_local(name)
                } else {
                    self.globals.get(name)
                };
                value.ok_or_else(|| {
                    self.runtime_error(*span, format!("undefined variable '{name}'"))
                })
            }
            Expr::Assign {
                name, value, span, ..
            } => {
                let val = self.eval(value)?;
                let assigned = if let Some(distance) = self.resolutions.get(*span) {
                    self.env.ancestor(distance).assign(name, val.clone())
                } else {
                    self.globals.assign(name, val.clone())
                };
                if assigned {
                    Ok(val)
                } else {
                    Err(self.runtime_error(*span, format!("undefined variable '{name}'")))
                }
            }
            Expr::Unary {
                op, right, span, ..
            } => {
                let value = self.eval(right)?;
                match op {
                    UnaryOp::Not => Ok(TwValue::Bool(value.is_falsey())),
                    UnaryOp::Neg => match value {
                        TwValue::Number(n) => Ok(TwValue::Number(-n)),
                        other => Err(self.runtime_error(
                            *span,
                            format!("operand must be a number, got {}", other.type_name()),
                        )),
                    },
                }
            }
            Expr::Binary {
                left,
                op,
                right,
                span,
            } => {
                let l = self.eval(left)?;
                let r = self.eval(right)?;
                self.binary(*op, l, r, *span)
            }
            Expr::Logical {
                left, op, right, ..
            } => {
                let l = self.eval(left)?;
                match op {
                    LogicalOp::Or if !l.is_falsey() => Ok(l),
                    LogicalOp::And if l.is_falsey() => Ok(l),
                    _ => self.eval(right),
                }
            }
            Expr::Call {
                callee,
                args,
                paren_span,
                ..
            } => {
                let callee_v = self.eval(callee)?;
                let mut arg_values = Vec::with_capacity(args.len());
                for a in args {
                    arg_values.push(self.eval(a)?);
                }
                self.call(callee_v, arg_values, *paren_span)
            }
            Expr::Get {
                object, name, span, ..
            } => {
                let object_v = self.eval(object)?;
                self.get_property(&object_v, name, *span)
            }
            Expr::Set {
                object,
                name,
                value,
                span,
            } => {
                let object_v = self.eval(object)?;
                let value_v = self.eval(value)?;
                match object_v {
                    TwValue::Instance(inst) => {
                        inst.borrow_mut()
                            .fields
                            .insert(name.clone(), value_v.clone());
                        Ok(value_v)
                    }
                    other => Err(self.runtime_error(
                        *span,
                        format!(
                            "can only set fields on instances, got {}",
                            other.type_name()
                        ),
                    )),
                }
            }
            Expr::This { span } => {
                if let Some(distance) = self.resolutions.get(*span) {
                    self.env
                        .ancestor(distance)
                        .get_local("this")
                        .ok_or_else(|| self.runtime_error(*span, "'this' used outside of a method"))
                } else {
                    Ok(self.globals.get("this").unwrap_or(TwValue::Nil))
                }
            }
            Expr::Super { method, span } => {
                let superclass = if let Some(distance) = self.resolutions.get(*span) {
                    match self.env.ancestor(distance).get_local("super") {
                        Some(TwValue::Class(c)) => c,
                        _ => return Err(self.runtime_error(*span, "'super' is not a class")),
                    }
                } else {
                    return Err(self.runtime_error(*span, "'super' used outside of a class"));
                };
                let instance = {
                    // `this` lives one scope inside `super`.
                    let distance = self.resolutions.get(*span).unwrap_or(0);
                    self.env
                        .ancestor(distance.saturating_sub(1))
                        .get_local("this")
                        .ok_or_else(|| {
                            self.runtime_error(*span, "'super' used outside of a method")
                        })?
                };
                let found = find_method(&superclass, method).ok_or_else(|| {
                    self.runtime_error(*span, format!("undefined property '{method}'"))
                })?;
                Ok(TwValue::Bound(Rc::new(TwBound {
                    receiver: instance,
                    method: found,
                })))
            }
        }
    }

    fn binary(&self, op: BinaryOp, l: TwValue, r: TwValue, span: Span) -> Result<TwValue, Error> {
        match op {
            BinaryOp::Eq => Ok(TwValue::Bool(l.equals(&r))),
            BinaryOp::Ne => Ok(TwValue::Bool(!l.equals(&r))),
            BinaryOp::Add => match (&l, &r) {
                (TwValue::Number(a), TwValue::Number(b)) => Ok(TwValue::Number(a + b)),
                (TwValue::Str(a), TwValue::Str(b)) => Ok(TwValue::Str(Rc::from(format!("{a}{b}")))),
                _ => Err(self.runtime_error(
                    span,
                    format!(
                        "operands must be two numbers or two strings, got {} and {}",
                        l.type_name(),
                        r.type_name()
                    ),
                )),
            },
            BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Lt
            | BinaryOp::Le => {
                let (a, b) = match (&l, &r) {
                    (TwValue::Number(a), TwValue::Number(b)) => (*a, *b),
                    _ => {
                        return Err(self.runtime_error(
                            span,
                            format!(
                                "operands must be numbers, got {} and {}",
                                l.type_name(),
                                r.type_name()
                            ),
                        ))
                    }
                };
                Ok(match op {
                    BinaryOp::Sub => TwValue::Number(a - b),
                    BinaryOp::Mul => TwValue::Number(a * b),
                    BinaryOp::Div => TwValue::Number(a / b),
                    BinaryOp::Gt => TwValue::Bool(a > b),
                    BinaryOp::Ge => TwValue::Bool(a >= b),
                    BinaryOp::Lt => TwValue::Bool(a < b),
                    BinaryOp::Le => TwValue::Bool(a <= b),
                    _ => unreachable!("handled above"),
                })
            }
        }
    }

    fn call(&mut self, callee: TwValue, args: Vec<TwValue>, span: Span) -> Result<TwValue, Error> {
        match callee {
            TwValue::Function(f) => self.call_function(&f, args, None, span),
            TwValue::Bound(b) => {
                self.call_function(&b.method, args, Some(b.receiver.clone()), span)
            }
            TwValue::Native(name, arity) => {
                if args.len() != arity {
                    return Err(self.runtime_error(
                        span,
                        format!("expected {arity} arguments but got {}", args.len()),
                    ));
                }
                match name {
                    "clock" => Ok(TwValue::Number(now_seconds())),
                    _ => Err(self.runtime_error(span, format!("unknown native '{name}'"))),
                }
            }
            TwValue::Class(class) => {
                let instance = Rc::new(RefCell::new(TwInstance {
                    class: Rc::clone(&class),
                    fields: HashMap::new(),
                }));
                if let Some(init) = find_method(&class, "init") {
                    self.call_function(
                        &init,
                        args,
                        Some(TwValue::Instance(instance.clone())),
                        span,
                    )?;
                } else if !args.is_empty() {
                    return Err(self.runtime_error(
                        span,
                        format!("expected 0 arguments but got {}", args.len()),
                    ));
                }
                Ok(TwValue::Instance(instance))
            }
            other => Err(self.runtime_error(
                span,
                format!(
                    "can only call functions and classes, got {}",
                    other.type_name()
                ),
            )),
        }
    }

    fn call_function(
        &mut self,
        function: &Rc<TwFunction>,
        args: Vec<TwValue>,
        this: Option<TwValue>,
        span: Span,
    ) -> Result<TwValue, Error> {
        if args.len() != function.params.len() {
            return Err(self.runtime_error(
                span,
                format!(
                    "expected {} arguments but got {}",
                    function.params.len(),
                    args.len()
                ),
            ));
        }

        let previous = self.env.clone();
        // Bind `this` in its own environment layer so that its lexical distance
        // matches the resolver's model (the receiver scope encloses the
        // parameters scope). This mirrors jlox's `LoxFunction::bind`.
        let enclosing = if let Some(receiver) = &this {
            let env = Environment::new(Some(function.closure.clone()));
            env.define("this", receiver.clone());
            env
        } else {
            function.closure.clone()
        };
        let call_env = Environment::new(Some(enclosing));
        for (param, value) in function.params.iter().zip(args) {
            call_env.define(param.clone(), value);
        }
        self.env = call_env;

        let result = (|| {
            for stmt in &function.body {
                if let Flow::Return(v) = self.exec(stmt)? {
                    return Ok(Some(v));
                }
            }
            Ok(None)
        })();
        self.env = previous;

        let returned = result?;
        if function.is_initializer {
            // Initializers always return the receiver.
            return Ok(this.unwrap_or(TwValue::Nil));
        }
        Ok(returned.unwrap_or(TwValue::Nil))
    }

    fn get_property(&self, object: &TwValue, name: &str, span: Span) -> Result<TwValue, Error> {
        match object {
            TwValue::Instance(inst) => {
                if let Some(v) = inst.borrow().fields.get(name) {
                    return Ok(v.clone());
                }
                if let Some(method) = find_method(&inst.borrow().class, name) {
                    return Ok(TwValue::Bound(Rc::new(TwBound {
                        receiver: object.clone(),
                        method,
                    })));
                }
                Err(self.runtime_error(span, format!("undefined property '{name}'")))
            }
            other => Err(self.runtime_error(
                span,
                format!("only instances have properties, got {}", other.type_name()),
            )),
        }
    }
}

/// Finds a method by name, walking the superclass chain.
fn find_method(class: &Rc<TwClass>, name: &str) -> Option<Rc<TwFunction>> {
    let mut current = Some(Rc::clone(class));
    while let Some(c) = current {
        if let Some(m) = c.methods.get(name) {
            return Some(Rc::clone(m));
        }
        current = c.superclass.clone();
    }
    None
}

/// Wall-clock seconds since the Unix epoch (best effort; 0.0 on error).
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Convenience entry point: resolve and run `program` on the tree-walker,
/// returning captured `print` output.
///
/// # Errors
///
/// Returns the first resolver or runtime error.
pub fn interpret(program: &[Stmt]) -> Result<String, Error> {
    let resolution = crate::resolver::resolve(program);
    if let Some(err) = resolution.errors.into_iter().next() {
        return Err(err);
    }
    let mut walker = TreeWalker::new(resolution.resolutions);
    walker.interpret(program)?;
    Ok(std::mem::take(&mut walker.output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn run(src: &str) -> String {
        let program = parse(src).expect("parse");
        interpret(&program).expect("no error")
    }

    fn run_err(src: &str) -> Error {
        let program = parse(src).expect("parse");
        interpret(&program).expect_err("expected error")
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(run("print 1 + 2 * 3;"), "7\n");
        assert_eq!(run("print (1 + 2) * 3;"), "9\n");
    }

    #[test]
    fn string_concat() {
        assert_eq!(run(r#"print "a" + "b";"#), "ab\n");
    }

    #[test]
    fn control_flow_and_loops() {
        assert_eq!(
            run("var s = 0; for (var i = 1; i <= 5; i = i + 1) s = s + i; print s;"),
            "15\n"
        );
        assert_eq!(run("if (true) print 1; else print 2;"), "1\n");
    }

    #[test]
    fn break_and_continue() {
        assert_eq!(
            run("var i = 0; var s = 0; while (i < 10) { i = i + 1; if (i == 3) continue; if (i == 6) break; s = s + i; } print s;"),
            "12\n"
        );
    }

    #[test]
    fn recursive_function() {
        assert_eq!(
            run("fun fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } print fib(10);"),
            "55\n"
        );
    }

    #[test]
    fn closures_capture_environment() {
        let src = "
            fun makeCounter() {
                var count = 0;
                fun increment() { count = count + 1; return count; }
                return increment;
            }
            var c = makeCounter();
            print c();
            print c();
            print c();
        ";
        assert_eq!(run(src), "1\n2\n3\n");
    }

    #[test]
    fn nested_closures() {
        let src = "
            fun outer() {
                var x = 1;
                fun middle() {
                    var y = 2;
                    fun inner() { return x + y; }
                    return inner;
                }
                return middle();
            }
            print outer()();
        ";
        assert_eq!(run(src), "3\n");
    }

    #[test]
    fn classes_and_inheritance() {
        let src = "
            class Animal {
                init(name) { this.name = name; }
                speak() { return this.name + \" makes a sound\"; }
            }
            class Dog < Animal {
                speak() { return super.speak() + \" (woof)\"; }
            }
            var d = Dog(\"Rex\");
            print d.speak();
        ";
        assert_eq!(run(src), "Rex makes a sound (woof)\n");
    }

    #[test]
    fn runtime_type_error() {
        let err = run_err("print 1 + true;");
        assert_eq!(err.category(), "runtime");
    }

    #[test]
    fn undefined_variable_error() {
        let err = run_err("print nope;");
        assert_eq!(err.category(), "runtime");
    }

    #[test]
    fn arity_mismatch_error() {
        let err = run_err("fun f(a) {} f(1, 2);");
        assert!(err.to_string().contains("1 arguments"));
    }
}
