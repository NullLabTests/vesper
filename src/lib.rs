//! # Vesper
//!
//! Vesper is a small, dynamically-typed programming language with two complete
//! execution back-ends that must agree on observable behavior:
//!
//! * [`environment`] — a tree-walking interpreter, used as an executable
//!   specification and cross-check oracle.
//! * [`vm`] — a stack-based bytecode virtual machine fed by [`compiler`].
//!
//! The pipeline is the classic multi-pass one:
//!
//! ```text
//! source ── lexer ──▶ tokens ── parser ──▶ AST ──┬─ resolver ──▶ scope distances ──▶ environment (tree-walker)
//!                                               │
//!                                               └─ compiler ──▶ Function/Chunk ──▶ vm
//! ```
//!
//! Errors at every stage are the single [`error::Error`] type carrying precise
//! [`span::Span`] locations.
//!
//! # Example
//!
//! ```
//! let output = vesper::run_source("print 6 * 7;", false).expect("no error");
//! assert_eq!(output, "42\n");
//! ```

#![forbid(unsafe_code)]
#![doc(html_root_url = "https://docs.rs/vesper")]

pub mod ast;
pub mod chunk;
pub mod compiler;
pub mod environment;
pub mod error;
pub mod lexer;
pub mod object;
pub mod opcode;
pub mod parser;
pub mod repl;
pub mod resolver;
pub mod span;
pub mod token;
pub mod value;
pub mod vm;

use crate::error::Error;
use crate::object::Function;
use std::rc::Rc;

/// Runs the static analysis pipeline (lex, parse, resolve, compile) and returns
/// the top-level script [`Function`].
///
/// # Errors
///
/// Returns the first lexing, parsing, resolution, or compilation error.
pub fn compile_source(source: &str) -> Result<Rc<Function>, Error> {
    let program = parser::parse(source).map_err(first_error)?;

    let resolution = resolver::resolve(&program);
    if let Some(error) = resolution.errors.into_iter().next() {
        return Err(error);
    }

    compiler::compile(&program, source).map_err(first_error)
}

/// Lexes, parses, resolves, compiles, and runs `source` on the bytecode VM,
/// returning the captured `print` output.
///
/// # Errors
///
/// Returns the first error from any stage.
pub fn run_source(source: &str, trace: bool) -> Result<String, Error> {
    let function = compile_source(source)?;
    let mut vm = vm::Vm::with_trace(trace);
    vm.interpret(function)
}

/// Lexes, parses, resolves, and runs `source` on the tree-walking interpreter,
/// returning the captured `print` output.
///
/// # Errors
///
/// Returns the first error from any stage.
pub fn run_tree_source(source: &str) -> Result<String, Error> {
    let program = parser::parse(source).map_err(first_error)?;
    environment::interpret(&program)
}

/// Convenience for turning a `Vec<Error>` (as produced by the lexer/parser/
/// compiler) into a single error.
fn first_error(errors: Vec<Error>) -> Error {
    errors.into_iter().next().unwrap_or_else(|| {
        Error::compile(
            "internal error: stage reported failure with no diagnostics",
            span::Span::dummy(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_simple_program() {
        assert_eq!(run_source("print 1 + 1;", false).unwrap(), "2\n");
    }

    #[test]
    fn vm_and_tree_walker_agree() {
        let program = "var total = 0; \
                       for (var i = 0; i < 10; i = i + 1) { total = total + i; } \
                       print total;";
        assert_eq!(
            run_source(program, false).unwrap(),
            run_tree_source(program).unwrap()
        );
    }

    #[test]
    fn reports_lex_error() {
        let err = compile_source("var x = @;").unwrap_err();
        assert_eq!(err.category(), "lex");
    }

    #[test]
    fn reports_parse_error() {
        let err = compile_source("var x = ;").unwrap_err();
        assert_eq!(err.category(), "parse");
    }
}
