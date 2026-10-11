//! Recursive-descent parser producing an [`crate::ast`] program.
//!
//! # Grammar
//!
//! The full grammar, in EBNF-ish notation (`|` alternation, `{}` repetition,
//! `[]` optional, terminals in quotes):
//!
//! ```text
//! program      -> declaration* EOF
//! declaration  -> classDecl | funDecl | varDecl | statement
//! classDecl    -> "class" IDENT ( "<" IDENT )? "{" function* "}"
//! funDecl      -> "fun" function
//! function     -> IDENT "(" parameters? ")" block
//! parameters   -> IDENT ( "," IDENT )*
//! varDecl      -> "var" IDENT ( "=" expression )? ";"
//! statement    -> exprStmt | forStmt | ifStmt | printStmt | returnStmt
//!               | breakStmt | continueStmt | whileStmt | block
//! exprStmt     -> expression ";"
//! forStmt      -> "for" "(" ( varDecl | exprStmt | ";" ) expression? ";" expression? ")" statement
//! ifStmt       -> "if" "(" expression ")" statement ( "else" statement )?
//! printStmt    -> "print" expression ";"
//! returnStmt   -> "return" expression? ";"
//! breakStmt    -> "break" ";"
//! continueStmt -> "continue" ";"
//! whileStmt    -> "while" "(" expression ")" statement
//! block        -> "{" declaration* "}"
//! expression   -> assignment
//! assignment   -> ( call "." )? IDENT "=" assignment | logic_or
//! logic_or     -> logic_and ( "or" logic_and )*
//! logic_and    -> equality ( "and" equality )*
//! equality     -> comparison ( ( "!=" | "==" ) comparison )*
//! comparison   -> term ( ( ">" | ">=" | "<" | "<=" ) term )*
//! term         -> factor ( ( "-" | "+" ) factor )*
//! factor       -> unary ( ( "/" | "*" ) unary )*
//! unary        -> ( "!" | "-" ) unary | call
//! call         -> primary ( "(" arguments? ")" | "." IDENT )*
//! arguments    -> expression ( "," expression )*
//! primary      -> NUMBER | STRING | "true" | "false" | "nil" | "this"
//!               | IDENT | "(" expression ")" | "super" "." IDENT
//! ```
//!
//! # Error recovery
//!
//! On a syntax error the parser records it and *synchronizes* by skipping tokens
//! until a likely statement boundary (a `;`, a keyword that starts a statement,
//! or EOF). This lets a single parse report multiple independent errors, which
//! is friendlier than stopping at the first one.

use crate::ast::{BinaryOp, Expr, LogicalOp, Stmt, UnaryOp};
use crate::error::Error;
use crate::token::{Literal, Token, TokenKind};

/// Maximum number of parameters a function may declare. See
/// [`crate::compiler::MAX_PARAMS`] for the rationale (byte-coded operand).
const MAX_PARAMS: usize = 255;

/// Result of a parse: the program (possibly partial) and every error found.
#[derive(Debug)]
pub struct ParseOutput {
    /// Parsed statements. Only meaningful when `errors` is empty.
    pub program: Vec<Stmt>,
    /// All syntax errors encountered, in source order.
    pub errors: Vec<Error>,
}

/// Recursive-descent parser. See the module docs for the grammar.
#[derive(Debug)]
pub struct Parser {
    tokens: Vec<Token>,
    current: usize,
    errors: Vec<Error>,
    loop_depth: usize,
}

/// Internal parse result: `Err(())` means an error was already recorded in
/// `self.errors`, avoiding the need to thread error values around.
type PResult<T> = Result<T, ()>;

impl Parser {
    /// Creates a parser over a token stream (which must end in EOF).
    #[must_use]
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            current: 0,
            errors: Vec::new(),
            loop_depth: 0,
        }
    }

    /// Parses a whole program. Never panics; all problems are reported as
    /// [`ParseOutput::errors`].
    #[must_use]
    pub fn parse(mut self) -> ParseOutput {
        let mut program = Vec::new();
        while !self.is_at_end() {
            match self.declaration() {
                Ok(stmt) => program.push(stmt),
                Err(()) => self.synchronize(),
            }
        }
        ParseOutput {
            program,
            errors: self.errors,
        }
    }

    // --- Declarations -----------------------------------------------------

    fn declaration(&mut self) -> PResult<Stmt> {
        if self.matches(&[TokenKind::Class]) {
            return self.class_declaration();
        }
        if self.matches(&[TokenKind::Fun]) {
            return self.function("function");
        }
        if self.matches(&[TokenKind::Var]) {
            return self.var_declaration();
        }
        self.statement()
    }

    fn class_declaration(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        let name = self.consume_ident("expect class name")?;
        let name_str = name.lexeme;

        let superclass = if self.matches(&[TokenKind::Less]) {
            let super_name = self.consume_ident("expect superclass name")?;
            Some(Expr::Variable {
                name: super_name.lexeme,
                span: super_name.span,
            })
        } else {
            None
        };

        self.consume(TokenKind::LeftBrace, "expect '{' before class body")?;
        let mut methods = Vec::new();
        while !self.check(TokenKind::RightBrace) && !self.is_at_end() {
            methods.push(self.function("method")?);
        }
        let end = self
            .consume(TokenKind::RightBrace, "expect '}' after class body")?
            .span;

        Ok(Stmt::Class {
            name: name_str,
            superclass,
            methods,
            span: start.merge(end),
        })
    }

    fn function(&mut self, what: &str) -> PResult<Stmt> {
        // `fun` (or class-method dispatch) has already been consumed.
        let name = self.consume_ident(&format!("expect {what} name"))?;
        let name_str = name.lexeme.clone();
        let start = name.span;

        self.consume(
            TokenKind::LeftParen,
            &format!("expect '(' after {what} name"),
        )?;
        let mut params: Vec<String> = Vec::new();
        if !self.check(TokenKind::RightParen) {
            loop {
                if params.len() >= MAX_PARAMS {
                    self.error_at_current(&format!(
                        "cannot have more than {MAX_PARAMS} parameters"
                    ));
                    return Err(());
                }
                let param = self.consume_ident("expect parameter name")?;
                params.push(param.lexeme.clone());
                if !self.matches(&[TokenKind::Comma]) {
                    break;
                }
            }
        }
        self.consume(TokenKind::RightParen, "expect ')' after parameters")?;
        self.consume(
            TokenKind::LeftBrace,
            &format!("expect '{{' before {what} body"),
        )?;
        // A function body is a fresh loop context: `break`/`continue` cannot
        // cross a function boundary.
        let saved_loop_depth = self.loop_depth;
        self.loop_depth = 0;
        let body = self.block_body()?;
        self.loop_depth = saved_loop_depth;
        let end = self.previous().span;

        Ok(Stmt::Function {
            name: name_str,
            params,
            body,
            span: start.merge(end),
        })
    }

    fn var_declaration(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        let name = self.consume_ident("expect variable name")?;
        let name_str = name.lexeme;

        let initializer = if self.matches(&[TokenKind::Equal]) {
            Some(self.expression()?)
        } else {
            None
        };

        let end = self
            .consume(
                TokenKind::Semicolon,
                "expect ';' after variable declaration",
            )?
            .span;
        Ok(Stmt::Var {
            name: name_str,
            initializer,
            span: start.merge(end),
        })
    }

    // --- Statements -------------------------------------------------------

    fn statement(&mut self) -> PResult<Stmt> {
        if self.matches(&[TokenKind::For]) {
            return self.for_statement();
        }
        if self.matches(&[TokenKind::If]) {
            return self.if_statement();
        }
        if self.matches(&[TokenKind::Print]) {
            return self.print_statement();
        }
        if self.matches(&[TokenKind::Return]) {
            return self.return_statement();
        }
        if self.matches(&[TokenKind::Break]) {
            return self.break_statement();
        }
        if self.matches(&[TokenKind::Continue]) {
            return self.continue_statement();
        }
        if self.matches(&[TokenKind::While]) {
            return self.while_statement();
        }
        if self.matches(&[TokenKind::LeftBrace]) {
            return self.block_statement();
        }
        self.expression_statement()
    }

    fn block_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        let statements = self.block_body()?;
        let end = self.previous().span;
        Ok(Stmt::Block {
            statements,
            span: start.merge(end),
        })
    }

    /// Parses statements up to (and consuming) the matching `}`.
    fn block_body(&mut self) -> PResult<Vec<Stmt>> {
        let mut statements = Vec::new();
        while !self.check(TokenKind::RightBrace) && !self.is_at_end() {
            match self.declaration() {
                Ok(stmt) => statements.push(stmt),
                Err(()) => self.synchronize(),
            }
        }
        self.consume(TokenKind::RightBrace, "expect '}' after block")?;
        Ok(statements)
    }

    fn for_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        self.consume(TokenKind::LeftParen, "expect '(' after 'for'")?;

        // Initializer clause.
        let initializer = if self.matches(&[TokenKind::Semicolon]) {
            None
        } else if self.matches(&[TokenKind::Var]) {
            Some(Box::new(self.var_declaration()?))
        } else {
            Some(Box::new(self.expression_statement()?))
        };

        // Condition clause (optional; `None` means `true`).
        let condition = if self.check(TokenKind::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        self.consume(TokenKind::Semicolon, "expect ';' after loop condition")?;

        // Increment clause (executed after each body iteration).
        let increment = if self.check(TokenKind::RightParen) {
            None
        } else {
            Some(self.expression()?)
        };
        self.consume(TokenKind::RightParen, "expect ')' after for clauses")?;

        self.loop_depth += 1;
        let body = self.statement()?;
        self.loop_depth -= 1;

        let span = start.merge(self.previous().span);
        Ok(Stmt::For {
            init: initializer,
            condition,
            increment,
            body: Box::new(body),
            span,
        })
    }

    fn if_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        self.consume(TokenKind::LeftParen, "expect '(' after 'if'")?;
        let condition = self.expression()?;
        self.consume(TokenKind::RightParen, "expect ')' after if condition")?;

        let then_branch = Box::new(self.statement()?);
        let else_branch = if self.matches(&[TokenKind::Else]) {
            Some(Box::new(self.statement()?))
        } else {
            None
        };
        let span = start.merge(self.previous().span);
        Ok(Stmt::If {
            condition,
            then_branch,
            else_branch,
            span,
        })
    }

    fn print_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        let expr = self.expression()?;
        let end = self
            .consume(TokenKind::Semicolon, "expect ';' after value")?
            .span;
        Ok(Stmt::Print {
            expr,
            span: start.merge(end),
        })
    }

    fn return_statement(&mut self) -> PResult<Stmt> {
        let keyword = self.previous().span;
        let value = if self.check(TokenKind::Semicolon) {
            None
        } else {
            Some(self.expression()?)
        };
        let end = self
            .consume(TokenKind::Semicolon, "expect ';' after return value")?
            .span;
        Ok(Stmt::Return {
            keyword_span: keyword,
            value,
            span: keyword.merge(end),
        })
    }

    fn break_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        let end = self
            .consume(TokenKind::Semicolon, "expect ';' after 'break'")?
            .span;
        if self.loop_depth == 0 {
            self.errors
                .push(Error::parse("'break' outside of a loop", start));
        }
        Ok(Stmt::Break {
            span: start.merge(end),
        })
    }

    fn continue_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        let end = self
            .consume(TokenKind::Semicolon, "expect ';' after 'continue'")?
            .span;
        if self.loop_depth == 0 {
            self.errors
                .push(Error::parse("'continue' outside of a loop", start));
        }
        Ok(Stmt::Continue {
            span: start.merge(end),
        })
    }

    fn while_statement(&mut self) -> PResult<Stmt> {
        let start = self.previous().span;
        self.consume(TokenKind::LeftParen, "expect '(' after 'while'")?;
        let condition = self.expression()?;
        self.consume(TokenKind::RightParen, "expect ')' after condition")?;
        self.loop_depth += 1;
        let body = self.statement()?;
        self.loop_depth -= 1;
        let span = start.merge(self.previous().span);
        Ok(Stmt::While {
            condition,
            body: Box::new(body),
            span,
        })
    }

    fn expression_statement(&mut self) -> PResult<Stmt> {
        let expr = self.expression()?;
        let end = self
            .consume(TokenKind::Semicolon, "expect ';' after expression")?
            .span;
        let span = expr.span().merge(end);
        Ok(Stmt::Expression { expr, span })
    }

    // --- Expressions ------------------------------------------------------

    fn expression(&mut self) -> PResult<Expr> {
        self.assignment()
    }

    fn assignment(&mut self) -> PResult<Expr> {
        let expr = self.logic_or()?;

        if self.matches(&[TokenKind::Equal]) {
            let equals = self.previous().span;
            let value = self.assignment()?;

            return match expr {
                Expr::Variable { name, span } => Ok(Expr::Assign {
                    name,
                    value: Box::new(value),
                    span: span.merge(equals),
                }),
                Expr::Get {
                    object, name, span, ..
                } => Ok(Expr::Set {
                    object,
                    name,
                    value: Box::new(value),
                    span: span.merge(equals),
                }),
                other => {
                    self.errors
                        .push(Error::parse("invalid assignment target", other.span()));
                    Err(())
                }
            };
        }
        Ok(expr)
    }

    fn logic_or(&mut self) -> PResult<Expr> {
        let mut expr = self.logic_and()?;
        while self.matches(&[TokenKind::Or]) {
            let right = self.logic_and()?;
            let span = expr.span().merge(right.span());
            expr = Expr::Logical {
                left: Box::new(expr),
                op: LogicalOp::Or,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn logic_and(&mut self) -> PResult<Expr> {
        let mut expr = self.equality()?;
        while self.matches(&[TokenKind::And]) {
            let right = self.equality()?;
            let span = expr.span().merge(right.span());
            expr = Expr::Logical {
                left: Box::new(expr),
                op: LogicalOp::And,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn equality(&mut self) -> PResult<Expr> {
        let mut expr = self.comparison()?;
        while self.matches(&[TokenKind::BangEqual, TokenKind::EqualEqual]) {
            let op = if self.previous().kind == TokenKind::BangEqual {
                BinaryOp::Ne
            } else {
                BinaryOp::Eq
            };
            let right = self.comparison()?;
            let span = expr.span().merge(right.span());
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let mut expr = self.term()?;
        while self.matches(&[
            TokenKind::Greater,
            TokenKind::GreaterEqual,
            TokenKind::Less,
            TokenKind::LessEqual,
        ]) {
            let op = match self.previous().kind {
                TokenKind::Greater => BinaryOp::Gt,
                TokenKind::GreaterEqual => BinaryOp::Ge,
                TokenKind::Less => BinaryOp::Lt,
                _ => BinaryOp::Le,
            };
            let right = self.term()?;
            let span = expr.span().merge(right.span());
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn term(&mut self) -> PResult<Expr> {
        let mut expr = self.factor()?;
        while self.matches(&[TokenKind::Minus, TokenKind::Plus]) {
            let op = if self.previous().kind == TokenKind::Minus {
                BinaryOp::Sub
            } else {
                BinaryOp::Add
            };
            let right = self.factor()?;
            let span = expr.span().merge(right.span());
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn factor(&mut self) -> PResult<Expr> {
        let mut expr = self.unary()?;
        while self.matches(&[TokenKind::Slash, TokenKind::Star]) {
            let op = if self.previous().kind == TokenKind::Slash {
                BinaryOp::Div
            } else {
                BinaryOp::Mul
            };
            let right = self.unary()?;
            let span = expr.span().merge(right.span());
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn unary(&mut self) -> PResult<Expr> {
        if self.matches(&[TokenKind::Bang, TokenKind::Minus]) {
            let op_tok = self.previous().clone();
            let op = if op_tok.kind == TokenKind::Bang {
                UnaryOp::Not
            } else {
                UnaryOp::Neg
            };
            let right = self.unary()?;
            let span = op_tok.span.merge(right.span());
            return Ok(Expr::Unary {
                op,
                right: Box::new(right),
                span,
            });
        }
        self.call()
    }

    fn call(&mut self) -> PResult<Expr> {
        let mut expr = self.primary()?;

        loop {
            if self.matches(&[TokenKind::LeftParen]) {
                expr = self.finish_call(expr)?;
            } else if self.matches(&[TokenKind::Dot]) {
                let name = self.consume_ident("expect property name after '.'")?;
                let span = expr.span().merge(name.span);
                expr = Expr::Get {
                    object: Box::new(expr),
                    name: name.lexeme.clone(),
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn finish_call(&mut self, callee: Expr) -> PResult<Expr> {
        let mut args = Vec::new();
        if !self.check(TokenKind::RightParen) {
            loop {
                if args.len() >= MAX_PARAMS {
                    self.error_at_current(&format!("cannot have more than {MAX_PARAMS} arguments"));
                    return Err(());
                }
                args.push(self.expression()?);
                if !self.matches(&[TokenKind::Comma]) {
                    break;
                }
            }
        }
        let paren = self.consume(TokenKind::RightParen, "expect ')' after arguments")?;
        let span = callee.span().merge(paren.span);
        Ok(Expr::Call {
            callee: Box::new(callee),
            args,
            paren_span: paren.span,
            span,
        })
    }

    fn primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        let expr = match tok.kind {
            TokenKind::False => {
                self.advance();
                Expr::Literal {
                    value: Literal::Bool(false),
                    span: tok.span,
                }
            }
            TokenKind::True => {
                self.advance();
                Expr::Literal {
                    value: Literal::Bool(true),
                    span: tok.span,
                }
            }
            TokenKind::Nil => {
                self.advance();
                Expr::Literal {
                    value: Literal::Nil,
                    span: tok.span,
                }
            }
            TokenKind::Number | TokenKind::String => {
                self.advance();
                let value = tok.literal.unwrap_or(Literal::Nil);
                Expr::Literal {
                    value,
                    span: tok.span,
                }
            }
            TokenKind::This => {
                self.advance();
                Expr::This { span: tok.span }
            }
            TokenKind::Identifier => {
                self.advance();
                Expr::Variable {
                    name: tok.lexeme.clone(),
                    span: tok.span,
                }
            }
            TokenKind::Super => {
                self.advance();
                let super_span = tok.span;
                self.consume(TokenKind::Dot, "expect '.' after 'super'")?;
                let method = self.consume_ident("expect superclass method name")?;
                Expr::Super {
                    method: method.lexeme.clone(),
                    span: super_span.merge(method.span),
                }
            }
            TokenKind::LeftParen => {
                self.advance();
                let inner = self.expression()?;
                let close = self.consume(TokenKind::RightParen, "expect ')' after expression")?;
                Expr::Grouping {
                    expression: Box::new(inner),
                    span: tok.span.merge(close.span),
                }
            }
            _ => {
                self.error_at_current("expect expression");
                return Err(());
            }
        };
        Ok(expr)
    }

    // --- Token helpers ----------------------------------------------------

    fn is_at_end(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current.min(self.tokens.len().saturating_sub(1))]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.current += 1;
        }
        self.previous()
    }

    fn check(&self, kind: TokenKind) -> bool {
        !self.is_at_end() && self.peek().kind == kind
    }

    fn matches(&mut self, kinds: &[TokenKind]) -> bool {
        if kinds.contains(&self.peek().kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume(&mut self, kind: TokenKind, message: &str) -> PResult<Token> {
        if self.check(kind) {
            Ok(self.advance().clone())
        } else {
            self.error_at_current(message);
            Err(())
        }
    }

    fn consume_ident(&mut self, message: &str) -> PResult<Token> {
        self.consume(TokenKind::Identifier, message)
    }

    fn error_at_current(&mut self, message: &str) {
        let span = self.peek().span;
        self.errors.push(Error::parse(message, span));
    }

    /// Skips tokens until a plausible statement boundary, to enable
    /// multi-error reporting.
    fn synchronize(&mut self) {
        if !self.is_at_end() {
            self.advance();
        }
        while !self.is_at_end() {
            if self.previous().kind == TokenKind::Semicolon {
                return;
            }
            match self.peek().kind {
                TokenKind::Class
                | TokenKind::Fun
                | TokenKind::Var
                | TokenKind::For
                | TokenKind::If
                | TokenKind::While
                | TokenKind::Print
                | TokenKind::Return => return,
                _ => {
                    self.advance();
                }
            }
        }
    }
}

/// Convenience entry point: lex and parse `source` in one call.
///
/// Returns the program on success, or all lexing/parsing errors on failure.
pub fn parse(source: &str) -> Result<Vec<Stmt>, Vec<Error>> {
    let tokens = crate::lexer::tokenize(source)?;
    let output = Parser::new(tokens).parse();
    if output.errors.is_empty() {
        Ok(output.program)
    } else {
        Err(output.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Vec<Stmt> {
        match parse(src) {
            Ok(p) => p,
            Err(errs) => panic!("unexpected parse errors: {errs:?}"),
        }
    }

    fn parse_err(src: &str) -> Vec<Error> {
        match parse(src) {
            Ok(p) => panic!("expected errors, got {p:?}"),
            Err(errs) => errs,
        }
    }

    #[test]
    fn parses_var_and_print() {
        let p = parse_ok("var a = 1; print a + 2;");
        assert_eq!(p.len(), 2);
        assert!(matches!(p[0], Stmt::Var { .. }));
        assert!(matches!(p[1], Stmt::Print { .. }));
    }

    #[test]
    fn parses_function_decl() {
        let p = parse_ok("fun add(a, b) { return a + b; }");
        assert!(matches!(&p[0], Stmt::Function { name, params, .. }
            if name == "add" && params.len() == 2));
    }

    #[test]
    fn parses_class_with_super() {
        let p = parse_ok("class B < A { m() { return 1; } }");
        match &p[0] {
            Stmt::Class {
                name,
                superclass,
                methods,
                ..
            } => {
                assert_eq!(name, "B");
                assert!(superclass.is_some());
                assert_eq!(methods.len(), 1);
            }
            other => panic!("expected class, got {other:?}"),
        }
    }

    #[test]
    fn parses_for_natively() {
        let p = parse_ok("for (var i = 0; i < 3; i = i + 1) print i;");
        match &p[0] {
            Stmt::For {
                init,
                condition,
                increment,
                ..
            } => {
                assert!(init.is_some());
                assert!(condition.is_some());
                assert!(increment.is_some());
            }
            other => panic!("expected for, got {other:?}"),
        }
    }

    #[test]
    fn assignment_is_right_associative() {
        let p = parse_ok("a = b = c;");
        match &p[0] {
            Stmt::Expression { expr, .. } => match expr {
                Expr::Assign { value, .. } => {
                    assert!(matches!(**value, Expr::Assign { .. }));
                }
                other => panic!("expected assign, got {other:?}"),
            },
            other => panic!("expected expr stmt, got {other:?}"),
        }
    }

    #[test]
    fn assignment_to_non_var_is_error() {
        let errs = parse_err("a + b = 3;");
        assert!(errs[0].to_string().contains("invalid assignment target"));
    }

    #[test]
    fn break_outside_loop_is_error() {
        let errs = parse_err("break;");
        assert!(errs
            .iter()
            .any(|e| e.to_string().contains("outside of a loop")));
    }

    #[test]
    fn reports_multiple_errors() {
        let errs = parse_err("var = ;\nprint ;\n");
        assert!(errs.len() >= 2, "expected multiple errors, got {errs:?}");
    }

    #[test]
    fn missing_semicolon_is_error() {
        let errs = parse_err("var a = 1\n");
        assert!(errs[0].to_string().contains("';'"));
    }

    #[test]
    fn too_many_params_is_error() {
        let params = (0..256)
            .map(|i| format!("p{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let src = format!("fun f({params}) {{}}");
        let errs = parse_err(&src);
        assert!(errs[0].to_string().contains("more than"));
    }
}
