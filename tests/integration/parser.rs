//! Parser integration tests: full-grammar acceptance, error recovery, and
//! robustness.

use proptest::prelude::*;
use vesper::ast::{Expr, Stmt};
use vesper::parser::parse;

#[test]
fn parses_a_full_program() {
    let source = r"
        class Base {
            init(x) { this.x = x; }
            get() { return this.x; }
        }
        class Derived < Base {
            init(x, y) { super.init(x); this.y = y; }
            get() { return super.get() + this.y; }
        }
        fun make(x) {
            fun closure() { return x; }
            return closure;
        }
        var total = 0;
        for (var i = 0; i < 10; i = i + 1) {
            if (i == 5) continue;
            total = total + i;
        }
        while (total > 0) { total = total - 1; break; }
        print make(3)();
    ";
    let program = parse(source).expect("program should parse");
    assert!(program.iter().any(|s| matches!(s, Stmt::Class { .. })));
    assert!(program.iter().any(|s| matches!(s, Stmt::Function { .. })));
    assert!(program.iter().any(|s| matches!(s, Stmt::For { .. })));
    assert!(program.iter().any(|s| matches!(s, Stmt::While { .. })));
}

#[test]
fn function_with_closure_body_parses() {
    let program = parse("fun outer() { var x = 1; fun inner() { return x; } return inner; }")
        .expect("should parse");
    let Stmt::Function { body, .. } = &program[0] else {
        panic!("expected function statement");
    };
    assert!(body.iter().any(|s| matches!(s, Stmt::Function { .. })));
}

#[test]
fn records_span_on_expression() {
    let program = parse("1 + 2;").unwrap();
    let Stmt::Expression { expr, .. } = &program[0] else {
        panic!("expected expression statement");
    };
    let Expr::Binary { span, .. } = expr else {
        panic!("expected binary expression");
    };
    assert_eq!(span.start, 0);
    assert_eq!(span.end, 5);
}

#[test]
fn collects_multiple_statement_errors() {
    // Two independent malformed statements: the parser should report both after
    // synchronizing on statement boundaries.
    let errors = parse("var a = ; var b = ;").unwrap_err();
    assert!(
        errors.len() >= 2,
        "expected error recovery to collect multiple errors, got {errors:?}"
    );
}

#[test]
fn rejects_trailing_garbage() {
    assert!(parse("print 1; @@@").is_err());
}

proptest! {
    /// The parser must never panic on arbitrary input (it may return errors).
    #[test]
    fn never_panics_on_arbitrary_input(source in "\\PC{0,96}") {
        let _ = parse(&source);
    }
}
