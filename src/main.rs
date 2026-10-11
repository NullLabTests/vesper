//! Vesper command-line interface.
//!
//! Usage:
//!
//! ```text
//! vesper [OPTIONS] [FILE]
//!
//! OPTIONS:
//!     --trace          Print the bytecode disassembly and stack after each instruction
//!     --disassemble    Print the compiled bytecode and exit
//!     --tree           Execute with the tree-walking interpreter instead of the VM
//!     -h, --help       Print help
//!     -V, --version    Print version
//! ```
//!
//! With no `FILE`, an interactive REPL is started. Exit codes follow the Lox
//! convention: `0` success, `65` static (lex/parse/resolve/compile) error,
//! `70` runtime error, `74` I/O error.

use std::process::ExitCode;

use vesper::chunk::disassemble_function;
use vesper::error::Error;
use vesper::vm::Vm;
use vesper::{compile_source, repl, run_tree_source};

const EXIT_OK: u8 = 0;
const EXIT_STATIC: u8 = 65;
const EXIT_RUNTIME: u8 = 70;
const EXIT_IO: u8 = 74;

fn main() -> ExitCode {
    ExitCode::from(run())
}

/// Parses arguments and dispatches, returning a process exit code.
fn run() -> u8 {
    let mut trace = false;
    let mut disassemble = false;
    let mut tree = false;
    let mut file: Option<String> = None;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return EXIT_OK;
            }
            "-V" | "--version" => {
                println!("vesper {}", env!("CARGO_PKG_VERSION"));
                return EXIT_OK;
            }
            "--trace" => trace = true,
            "--disassemble" => disassemble = true,
            "--tree" => tree = true,
            other if other.starts_with('-') && other != "-" => {
                eprintln!("vesper: unknown option '{other}'");
                eprintln!("try 'vesper --help'");
                return EXIT_STATIC;
            }
            other => {
                if file.is_some() {
                    eprintln!("vesper: at most one file may be given");
                    return EXIT_STATIC;
                }
                file = Some(other.to_string());
            }
        }
    }

    match file {
        Some(path) => run_file(&path, trace, disassemble, tree),
        None => match repl::Repl::new().and_then(|mut r| r.run()) {
            Ok(()) => EXIT_OK,
            Err(error) => {
                eprintln!("vesper: {error}");
                EXIT_IO
            }
        },
    }
}

/// Reads, optionally disassembles, and executes a file.
fn run_file(path: &str, trace: bool, disassemble: bool, tree: bool) -> u8 {
    let Ok(source) = std::fs::read_to_string(path) else {
        eprintln!("vesper: could not read '{path}'");
        return EXIT_IO;
    };

    if disassemble {
        return match compile_source(&source) {
            Ok(function) => {
                print_disassembly(&function);
                EXIT_OK
            }
            Err(error) => {
                eprint!("{}", error.render(path, &source));
                EXIT_STATIC
            }
        };
    }

    if tree {
        return match run_tree_source(&source) {
            Ok(output) => {
                print!("{output}");
                EXIT_OK
            }
            Err(error) => report(&error, path, &source),
        };
    }

    let result = compile_source(&source).and_then(|function| {
        let mut vm = Vm::with_trace(trace);
        vm.interpret(function)
    });
    match result {
        Ok(output) => {
            print!("{output}");
            EXIT_OK
        }
        Err(error) => report(&error, path, &source),
    }
}

/// Prints a diagnostic to stderr and maps the error to an exit code.
fn report(error: &Error, path: &str, source: &str) -> u8 {
    eprint!("{}", error.render(path, source));
    match error {
        Error::Runtime { .. } => EXIT_RUNTIME,
        Error::Io(_) => EXIT_IO,
        _ => EXIT_STATIC,
    }
}

/// Disassembles `function` and its nested functions.
fn print_disassembly(function: &vesper::object::Function) {
    let mut out = String::new();
    disassemble_function(function, &mut out);
    print!("{out}");
}

/// Prints usage information.
fn print_help() {
    println!(
        "\
vesper {version}
A small dynamically-typed language with a bytecode virtual machine.

USAGE:
    vesper [OPTIONS] [FILE]

With no FILE, starts an interactive REPL.

OPTIONS:
    --trace          Print disassembly and stack after each instruction
    --disassemble    Print the compiled bytecode and exit
    --tree           Use the tree-walking interpreter instead of the VM
    -h, --help       Print this help
    -V, --version    Print version

EXIT CODES:
    0   success
    65  static error (lex/parse/resolve/compile)
    70  runtime error
    74  I/O error",
        version = env!("CARGO_PKG_VERSION")
    );
}
