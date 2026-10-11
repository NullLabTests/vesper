//! Integration tests for the Vesper implementation.
//!
//! These tests drive the public library API end-to-end (lexer, parser,
//! compiler, and VM) rather than individual private helpers, complementing the
//! per-module unit tests.

mod compiler;
mod lexer;
mod parser;
mod vm;
