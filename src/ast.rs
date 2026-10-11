//! Abstract syntax tree.
//!
//! The AST is produced by the [`crate::parser`], consumed by the tree-walking
//! interpreter ([`crate::environment`]), the scope resolver
//! ([`crate::resolver`]), and the bytecode compiler ([`crate::compiler`]).
//!
//! # Design
//!
//! * Every node carries a [`Span`], enabling precise diagnostics at every
//!   later stage without re-deriving positions.
//! * Recursive positions are boxed ([`Box`]) to keep nodes fixed-size.
//! * `for` loops are desugared into `while` loops during parsing, so the AST
//!   (and therefore the compiler) has one canonical looping construct.

use crate::span::Span;
use crate::token::Literal;

/// Unary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    /// Arithmetic negation (`-`).
    Neg,
    /// Logical negation (`!`).
    Not,
}

/// Binary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+` (numeric addition or string concatenation).
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
}

impl BinaryOp {
    /// Human-readable operator symbol.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Lt => "<",
            Self::Le => "<=",
        }
    }
}

/// Short-circuiting logical operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalOp {
    /// `and`.
    And,
    /// `or`.
    Or,
}

/// An expression node.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// A literal constant.
    Literal {
        /// The literal value.
        value: Literal,
        /// Source span.
        span: Span,
    },
    /// A variable reference.
    Variable {
        /// The variable's name.
        name: String,
        /// Source span.
        span: Span,
    },
    /// An assignment expression; evaluates to the assigned value.
    Assign {
        /// The target variable's name.
        name: String,
        /// The value to assign.
        value: Box<Expr>,
        /// Source span covering the whole assignment.
        span: Span,
    },
    /// A unary operation.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// The operand.
        right: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// A binary operation.
    Binary {
        /// Left operand.
        left: Box<Expr>,
        /// The operator.
        op: BinaryOp,
        /// Right operand.
        right: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// A short-circuiting logical operation.
    Logical {
        /// Left operand.
        left: Box<Expr>,
        /// The operator.
        op: LogicalOp,
        /// Right operand.
        right: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// A parenthesized expression (value-identical to its inner expression).
    Grouping {
        /// The inner expression.
        expression: Box<Expr>,
        /// Source span (includes the parentheses).
        span: Span,
    },
    /// A function or method call.
    Call {
        /// The callee (any expression that evaluates to a callable).
        callee: Box<Expr>,
        /// Argument expressions.
        args: Vec<Expr>,
        /// Span of the closing parenthesis (for callee diagnostics).
        paren_span: Span,
        /// Source span covering the whole call.
        span: Span,
    },
    /// A property read (`object.name`).
    Get {
        /// The object expression.
        object: Box<Expr>,
        /// The property name.
        name: String,
        /// Source span.
        span: Span,
    },
    /// A property write (`object.name = value`).
    Set {
        /// The object expression.
        object: Box<Expr>,
        /// The property name.
        name: String,
        /// The value to store.
        value: Box<Expr>,
        /// Source span.
        span: Span,
    },
    /// The `this` receiver inside a method.
    This {
        /// Source span.
        span: Span,
    },
    /// A `super.method` access inside a subclass method.
    Super {
        /// The method name.
        method: String,
        /// Source span.
        span: Span,
    },
}

impl Expr {
    /// Returns the source span of this expression.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Literal { span, .. }
            | Self::Variable { span, .. }
            | Self::Assign { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Logical { span, .. }
            | Self::Grouping { span, .. }
            | Self::Call { span, .. }
            | Self::Get { span, .. }
            | Self::Set { span, .. }
            | Self::This { span, .. }
            | Self::Super { span, .. } => *span,
        }
    }
}

/// A statement node.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// An expression evaluated for its side effects.
    Expression {
        /// The expression.
        expr: Expr,
        /// Source span.
        span: Span,
    },
    /// `print expr;`
    Print {
        /// The expression whose value is printed.
        expr: Expr,
        /// Source span.
        span: Span,
    },
    /// `var name = init;` (`init` is optional).
    Var {
        /// The variable name.
        name: String,
        /// Optional initializer.
        initializer: Option<Expr>,
        /// Source span.
        span: Span,
    },
    /// A brace-delimited block introducing a new lexical scope.
    Block {
        /// Statements within the block.
        statements: Vec<Stmt>,
        /// Source span.
        span: Span,
    },
    /// `if (cond) then [else otherwise]`.
    If {
        /// The condition.
        condition: Expr,
        /// The `then` branch.
        then_branch: Box<Stmt>,
        /// The optional `else` branch.
        else_branch: Option<Box<Stmt>>,
        /// Source span.
        span: Span,
    },
    /// `while (cond) body`.
    While {
        /// The loop condition.
        condition: Expr,
        /// The loop body.
        body: Box<Stmt>,
        /// Source span.
        span: Span,
    },
    /// A `for` loop, kept native (not desugared) so that `continue` can run the
    /// increment clause.
    For {
        /// Optional initializer (`var i = 0` or an expression statement).
        init: Option<Box<Stmt>>,
        /// Optional condition (defaults to `true`).
        condition: Option<Expr>,
        /// Optional increment expression.
        increment: Option<Expr>,
        /// The loop body.
        body: Box<Stmt>,
        /// Source span.
        span: Span,
    },
    /// A function declaration.
    Function {
        /// The function name.
        name: String,
        /// Parameter names.
        params: Vec<String>,
        /// The body statements.
        body: Vec<Stmt>,
        /// Source span.
        span: Span,
    },
    /// `return [value];`
    Return {
        /// Span of the `return` keyword.
        keyword_span: Span,
        /// Optional return value.
        value: Option<Expr>,
        /// Source span.
        span: Span,
    },
    /// `break;`
    Break {
        /// Source span.
        span: Span,
    },
    /// `continue;`
    Continue {
        /// Source span.
        span: Span,
    },
    /// A class declaration.
    Class {
        /// The class name.
        name: String,
        /// The optional superclass expression.
        superclass: Option<Expr>,
        /// Method declarations (each a [`Stmt::Function`]).
        methods: Vec<Stmt>,
        /// Source span.
        span: Span,
    },
}

impl Stmt {
    /// Returns the source span of this statement.
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Expression { span, .. }
            | Self::Print { span, .. }
            | Self::Var { span, .. }
            | Self::Block { span, .. }
            | Self::If { span, .. }
            | Self::While { span, .. }
            | Self::For { span, .. }
            | Self::Function { span, .. }
            | Self::Return { span, .. }
            | Self::Break { span, .. }
            | Self::Continue { span, .. }
            | Self::Class { span, .. } => *span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_accessor_works() {
        let e = Expr::Literal {
            value: Literal::Number(1.0),
            span: Span::new(2, 5),
        };
        assert_eq!(e.span(), Span::new(2, 5));

        let s = Stmt::Break {
            span: Span::new(9, 10),
        };
        assert_eq!(s.span(), Span::new(9, 10));
    }

    #[test]
    fn binary_symbols() {
        assert_eq!(BinaryOp::Add.symbol(), "+");
        assert_eq!(BinaryOp::Le.symbol(), "<=");
    }
}
