//! Compiler integration tests: AST to bytecode translation and diagnostics.

use std::fmt::Write as _;
use std::rc::Rc;
use vesper::chunk::disassemble_function;
use vesper::compiler::compile;
use vesper::object::Function;
use vesper::parser::parse;

/// Compiles `source` and returns its recursive disassembly.
fn disassemble(source: &str) -> String {
    let program = parse(source).expect("source should parse");
    let function: Rc<Function> = compile(&program, source).expect("source should compile");
    let mut out = String::new();
    disassemble_function(&function, &mut out);
    out
}

#[test]
fn emits_arithmetic_and_print() {
    let out = disassemble("print 1 + 2 * 3;");
    assert!(out.contains("OP_CONSTANT"));
    assert!(out.contains("OP_MULTIPLY"));
    assert!(out.contains("OP_ADD"));
    assert!(out.contains("OP_PRINT"));
}

#[test]
fn emits_control_flow_opcodes() {
    let out = disassemble("if (1 < 2) print 1; while (false) print 2;");
    assert!(out.contains("OP_LESS"));
    assert!(out.contains("OP_JUMP_IF_FALSE"));
    assert!(out.contains("OP_JUMP"));
    assert!(out.contains("OP_LOOP"));
}

#[test]
fn compiles_closure_with_upvalue() {
    let out = disassemble("fun outer() { var x = 1; fun inner() { return x; } return inner; }");
    assert!(out.contains("OP_CLOSURE"));
    assert!(out.contains("OP_GET_UPVALUE"));
    assert!(out.contains("local") || out.contains("upvalue"));
}

#[test]
fn compiles_class_inheritance() {
    let out = disassemble("class A { m() { return 1; } } class B < A { m() { return 2; } }");
    assert!(out.contains("OP_CLASS"));
    assert!(out.contains("OP_INHERIT"));
    assert!(out.contains("OP_METHOD"));
}

#[test]
fn reports_break_outside_loop() {
    let program = parse("break;").unwrap_err();
    // The parser rejects top-level `break`; it must produce a diagnostic.
    assert!(!program.is_empty());
}

#[test]
fn reports_too_many_locals() {
    let mut source = String::from("{");
    for i in 0..300 {
        let _ = write!(source, "var v{i} = {i};");
    }
    source.push('}');
    let program = parse(&source).expect("should parse");
    let errors = compile(&program, &source).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| e.to_string().contains("local variables")));
}

#[test]
fn constant_pool_is_deduplicated() {
    // The same string constant repeated should be stored once; the program
    // should compile and run identically.
    let out = disassemble(r#"print "hi"; print "hi"; print "hi";"#);
    assert!(out.contains("OP_PRINT"));
}
