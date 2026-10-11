//! Static scope resolution and semantic analysis.
//!
//! This pass walks the AST *before* execution/compilation and performs the
//! checks that require knowledge of the lexical environment:
//!
//! * Redeclaration of a variable in the same scope.
//! * Reading a local variable in its own initializer.
//! * `return` outside a function (or with a value inside an initializer).
//! * `this`/`super` used outside a class (and `super` without a superclass).
//!
//! It also computes a [`ResolutionMap`] from each variable-reference span to
//! its **lexical distance** — the number of enclosing scopes the reference is
//! from the binding. The tree-walking interpreter uses these distances to find
//! the correct environment; the bytecode compiler performs an equivalent
//! resolution internally (see [`crate::compiler`] and `DESIGN.md` for why the
//! two are kept independent).

use crate::ast::{Expr, Stmt};
use crate::error::Error;
use crate::span::Span;
use std::collections::HashMap;

/// Maps a variable reference (identified by its span) to its lexical distance.
///
/// A distance of `0` means the binding is in the innermost scope; `1` means one
/// scope out, and so on. References absent from the map are globals.
#[derive(Debug, Default, Clone)]
pub struct ResolutionMap {
    distances: HashMap<Span, usize>,
}

impl ResolutionMap {
    /// Returns the lexical distance for a reference, if it is a local.
    #[must_use]
    pub fn get(&self, span: Span) -> Option<usize> {
        self.distances.get(&span).copied()
    }

    /// Inserts a resolution.
    fn insert(&mut self, span: Span, distance: usize) {
        self.distances.insert(span, distance);
    }
}

/// The output of resolution: the distance map and any diagnostics.
#[derive(Debug, Default)]
pub struct ResolveOutput {
    /// Lexical distances for local variable references.
    pub resolutions: ResolutionMap,
    /// Semantic errors found.
    pub errors: Vec<Error>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FunctionType {
    None,
    Function,
    Method,
    Initializer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassType {
    None,
    Class,
    Subclass,
}

/// The resolver state machine.
struct Resolver {
    scopes: Vec<HashMap<String, bool>>,
    resolutions: ResolutionMap,
    errors: Vec<Error>,
    current_function: FunctionType,
    current_class: ClassType,
}

impl Resolver {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            resolutions: ResolutionMap::default(),
            errors: Vec::new(),
            current_function: FunctionType::None,
            current_class: ClassType::None,
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(Error::compile(message, span));
    }

    fn begin_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, span: Span) {
        let Some(scope) = self.scopes.last_mut() else {
            return; // Global scope: no duplicate checking needed here.
        };
        if scope.contains_key(name) {
            self.error(
                span,
                format!("variable '{name}' already declared in this scope"),
            );
            return;
        }
        scope.insert(name.to_string(), false);
    }

    fn define(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), true);
        }
    }

    /// Records the distance to a local binding, if `name` resolves in an
    /// enclosing scope.
    fn resolve_local(&mut self, name: &str, span: Span) {
        // Iterate from innermost to outermost.
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if scope.contains_key(name) {
                let distance = self.scopes.len() - 1 - i;
                // Reading before initialization is an error.
                if scope.get(name) == Some(&false) {
                    self.error(
                        span,
                        format!("cannot read local variable '{name}' in its own initializer"),
                    );
                }
                self.resolutions.insert(span, distance);
                return;
            }
        }
        // Not found: global. No entry recorded.
    }

    fn resolve_statements(&mut self, statements: &[Stmt]) {
        for stmt in statements {
            self.resolve_stmt(stmt);
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expression { expr, .. } => self.resolve_expr(expr),
            Stmt::Print { expr, .. } => self.resolve_expr(expr),
            Stmt::Var {
                name,
                initializer,
                span,
            } => {
                self.declare(name, *span);
                if let Some(init) = initializer {
                    self.resolve_expr(init);
                }
                self.define(name);
            }
            Stmt::Block { statements, .. } => {
                self.begin_scope();
                self.resolve_statements(statements);
                self.end_scope();
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.resolve_expr(condition);
                self.resolve_stmt(then_branch);
                if let Some(else_b) = else_branch {
                    self.resolve_stmt(else_b);
                }
            }
            Stmt::While {
                condition, body, ..
            } => {
                self.resolve_expr(condition);
                self.resolve_stmt(body);
            }
            Stmt::For {
                init,
                condition,
                increment,
                body,
                ..
            } => {
                self.begin_scope();
                if let Some(init) = init {
                    self.resolve_stmt(init);
                }
                if let Some(cond) = condition {
                    self.resolve_expr(cond);
                }
                if let Some(inc) = increment {
                    self.resolve_expr(inc);
                }
                self.resolve_stmt(body);
                self.end_scope();
            }
            Stmt::Function {
                name,
                params,
                body,
                span,
            } => {
                self.declare(name, *span);
                self.define(name);
                self.resolve_function(params, body, FunctionType::Function);
            }
            Stmt::Return {
                keyword_span,
                value,
                ..
            } => {
                if self.current_function == FunctionType::None {
                    self.error(*keyword_span, "cannot return from top-level code");
                }
                if let Some(v) = value {
                    if self.current_function == FunctionType::Initializer {
                        self.error(*keyword_span, "cannot return a value from an initializer");
                    }
                    self.resolve_expr(v);
                }
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {
                // Loop nesting was validated by the parser.
            }
            Stmt::Class {
                name,
                superclass,
                methods,
                span,
            } => self.resolve_class(name, superclass.as_ref(), methods, *span),
        }
    }

    fn resolve_function(&mut self, params: &[String], body: &[Stmt], kind: FunctionType) {
        let enclosing = self.current_function;
        self.current_function = kind;

        self.begin_scope();
        for p in params {
            self.declare(p, Span::dummy());
            self.define(p);
        }
        self.resolve_statements(body);
        self.end_scope();

        self.current_function = enclosing;
    }

    fn resolve_class(
        &mut self,
        name: &str,
        superclass: Option<&Expr>,
        methods: &[Stmt],
        span: Span,
    ) {
        let enclosing = self.current_class;
        self.current_class = ClassType::Class;

        self.declare(name, span);
        self.define(name);

        if let Some(super_expr) = superclass {
            self.current_class = ClassType::Subclass;
            // Validate that the superclass names a class (best effort: a
            // variable that is definitely not a class name cannot be checked
            // statically without a type system, so we only resolve it).
            self.resolve_expr(super_expr);

            // `super` bindings live in a scope enclosing the methods.
            self.begin_scope();
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert("super".to_string(), true);
            }
        }

        // `this` bindings live in a scope enclosing the methods.
        self.begin_scope();
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert("this".to_string(), true);
        }

        for method in methods {
            if let Stmt::Function {
                name: method_name,
                params,
                body,
                ..
            } = method
            {
                let kind = if method_name == "init" {
                    FunctionType::Initializer
                } else {
                    FunctionType::Method
                };
                self.resolve_function(params, body, kind);
            }
        }

        self.end_scope(); // `this`
        if superclass.is_some() {
            self.end_scope(); // `super`
        }

        self.current_class = enclosing;
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal { .. } => {}
            Expr::This { span } => {
                if self.current_class == ClassType::None {
                    self.error(*span, "cannot use 'this' outside of a class");
                } else {
                    // Record the distance to the synthetic `this` binding so the
                    // tree-walker can find the receiver.
                    self.resolve_local("this", *span);
                }
            }
            Expr::Variable { name, span } => self.resolve_local(name, *span),
            Expr::Assign {
                name, value, span, ..
            } => {
                self.resolve_expr(value);
                self.resolve_local(name, *span);
            }
            Expr::Unary { right, .. } => self.resolve_expr(right),
            Expr::Binary { left, right, .. } | Expr::Logical { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::Grouping { expression, .. } => self.resolve_expr(expression),
            Expr::Call { callee, args, .. } => {
                self.resolve_expr(callee);
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::Get { object, .. } => self.resolve_expr(object),
            Expr::Set { object, value, .. } => {
                self.resolve_expr(value);
                self.resolve_expr(object);
            }
            Expr::Super { method: _, span } => {
                if self.current_class == ClassType::None {
                    self.error(*span, "cannot use 'super' outside of a class");
                } else if self.current_class != ClassType::Subclass {
                    self.error(*span, "cannot use 'super' in a class with no superclass");
                }
                // Resolve as a variable reference to the synthetic `super`
                // binding so the interpreter can find it.
                self.resolve_local("super", *span);
            }
        }
    }
}

/// Resolves `program`, returning lexical distances and semantic errors.
#[must_use]
pub fn resolve(program: &[Stmt]) -> ResolveOutput {
    let mut resolver = Resolver::new();
    resolver.resolve_statements(program);
    ResolveOutput {
        resolutions: resolver.resolutions,
        errors: resolver.errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn resolve_src(src: &str) -> ResolveOutput {
        let program = parse(src).expect("parse");
        resolve(&program)
    }

    #[test]
    fn resolves_local_distance() {
        // `x` at depth 0 inside the block, referenced one scope out.
        let out = resolve_src("{ var x = 1; { print x; } }");
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        // There is exactly one variable reference (`x`).
        assert_eq!(out.resolutions.distances.len(), 1);
    }

    #[test]
    fn globals_are_not_resolved_locally() {
        let out = resolve_src("var x = 1; print x;");
        assert!(out.errors.is_empty());
        assert_eq!(out.resolutions.distances.len(), 0);
    }

    #[test]
    fn redeclaration_is_error() {
        let out = resolve_src("{ var a = 1; var a = 2; }");
        assert!(out
            .errors
            .iter()
            .any(|e| e.to_string().contains("already declared")));
    }

    #[test]
    fn read_in_own_initializer_is_error() {
        let out = resolve_src("{ var a = a; }");
        assert!(out
            .errors
            .iter()
            .any(|e| e.to_string().contains("its own initializer")));
    }

    #[test]
    fn return_at_top_level_is_error() {
        let out = resolve_src("return 1;");
        assert!(out
            .errors
            .iter()
            .any(|e| e.to_string().contains("top-level")));
    }

    #[test]
    fn this_outside_class_is_error() {
        let out = resolve_src("print this;");
        assert!(out.errors.iter().any(|e| e.to_string().contains("'this'")));
    }

    #[test]
    fn super_without_superclass_is_error() {
        let out = resolve_src("class A { m() { super.m(); } }");
        assert!(out
            .errors
            .iter()
            .any(|e| e.to_string().contains("no superclass")));
    }

    #[test]
    fn valid_class_resolves_cleanly() {
        let out = resolve_src("class A {} class B < A { m() { return this; } }");
        assert!(out.errors.is_empty(), "{:?}", out.errors);
    }
}
