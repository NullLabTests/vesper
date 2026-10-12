# Vesper

Vesper is a small, dynamically-typed programming language with **two complete
execution back-ends** that are required to agree on observable behavior:

* a **tree-walking interpreter** (`environment.rs`), used as an executable
  specification and cross-check oracle, and
* a **stack-based bytecode virtual machine** (`vm.rs`) fed by a real compiler
  (`compiler.rs`).

The project is an educational-yet-production-quality implementation: it takes
correctness, typed error handling, span-precise diagnostics, and test coverage
seriously while staying small enough to read end-to-end.

```text
source ── lexer ──▶ tokens ── parser ──▶ AST ──┬─ resolver ──▶ scope distances ──▶ environment (tree-walker)
                                              │
                                              └─ compiler ──▶ Function/Chunk ──▶ vm
```

## Features

* Dynamically typed values: `f64` numbers, booleans, `nil`, strings, functions,
  closures, classes, and instances.
* Lexical scoping, first-class functions, and closures with correct upvalue
  capture (including nested captures).
* Classes with fields, methods, initializers, single inheritance, `this`, and
  `super`.
* `if`/`else`, `while`, C-style `for`, `return`, `break`, `continue`, and
  short-circuiting `and`/`or`.
* Precise, span-carrying errors at every stage with a caret-underlined
  diagnostic renderer.
* A REPL, file execution, `--trace`, `--disassemble`, and `--tree` modes.

## Quick start

```console
$ cargo build --release
$ ./target/release/vesper examples/fib.vesper
0
1
1
2
3
5
8
13
21
34
55
```

Start the REPL with no arguments:

```console
$ cargo run
vesper> var greeting = "hello";
vesper> print greeting + ", world!";
hello, world!
```

## Command line

```text
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
    74  I/O error
```

## Language tour

```lox
// functions and recursion
fun fib(n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}

// closures capture their environment
fun make_counter() {
  var count = 0;
  fun increment() { count = count + 1; return count; }
  return increment;
}
var next = make_counter();
print next(); // 1
print next(); // 2

// classes and inheritance
class Animal {
  init(name) { this.name = name; }
  speak() { return this.name + " makes a sound"; }
}
class Dog < Animal {
  speak() { return this.name + " barks"; }
}
print Dog("Rex").speak(); // Rex barks
```

See `examples/` for runnable programs and `tests/fixtures/` for the golden test
corpus.

## Using the library

```rust
let output = vesper::run_source("print 6 * 7;", false).unwrap();
assert_eq!(output, "42\n");
```

The public entry points are:

| Function | Purpose |
| --- | --- |
| `vesper::run_source` | Compile and run on the bytecode VM, returning `print` output |
| `vesper::run_tree_source` | Parse and run on the tree-walking interpreter |
| `vesper::compile_source` | Run the static pipeline and return the top-level `Function` |
| `vesper::lexer::tokenize` | Lexing only |
| `vesper::parser::parse` | Parsing only |
| `vesper::resolver::resolve` | Scope resolution / semantic diagnostics |
| `vesper::compiler::compile` | AST → bytecode |
| `vesper::chunk::disassemble_function` | Recursive human-readable disassembly |

## Building, testing, benchmarking

```console
$ cargo build
$ cargo test                     # unit + integration + doc tests
$ cargo clippy --all-targets --all-features -- -D warnings
$ cargo fmt --all -- --check
$ cargo bench                    # criterion benchmarks for the VM
```

The CI workflow (`.github/workflows/ci.yml`) runs exactly the above on every
push and pull request, using `RUSTFLAGS="-D warnings"`.

Benchmarks (`benches/vm_bench.rs`) cover the three workloads named in the
specification: recursive Fibonacci, closure-heavy code, and a tight numeric
loop.

## Design highlights

* **Stack-based bytecode VM.** Stack machines have a trivially correct frame
  discipline and a small, uniform instruction encoding, which makes the
  disassembler and the invariants easy to audit. See `DESIGN.md` for the
  register-vs-stack discussion.
* **Two independent scope engines.** `resolver.rs` computes lexical distances
  for the tree-walker *and* produces semantic diagnostics; `compiler.rs`
  independently computes local slots and upvalues in the classic clox fashion.
  Agreement between the two back-ends is asserted on every fixture.
* **Reference counting, no GC.** `Rc`/`RefCell` give a deterministic memory
  model with no collection pauses; cycles are a documented non-collected
  limitation. `unsafe_code` is `deny`d crate-wide, with a single audited
  exception (`src/stack.rs`) for the interpreter's hot value stack.
* **Typed errors everywhere.** A single `error::Error` enum carries a `Span`;
  no stage swallows, panics, or prints. Every `?` is a typed, located failure.
* **Named limits.** `MAX_PARAMS`, `MAX_LOCALS`, `MAX_UPVALUES`, `MAX_FRAMES`,
  `STACK_MAX`, and `MAX_CONSTANTS` are documented constants with rationale
  rather than magic numbers.

Detailed architecture, invariants, and tradeoffs live in
[`DESIGN.md`](DESIGN.md). Numbered requirements and their traceability are in
[`REQUIREMENTS.md`](REQUIREMENTS.md), and how each is verified is in
[`VERIFICATION.md`](VERIFICATION.md).

## Repository layout

```text
vesper/
├── Cargo.toml
├── rust-toolchain.toml
├── README.md / DESIGN.md / REQUIREMENTS.md / VERIFICATION.md
├── LICENSE
├── .github/workflows/ci.yml
├── benches/vm_bench.rs
├── examples/{fib,closure,class}.vesper
├── src/
│   ├── lib.rs / main.rs
│   ├── error.rs / span.rs / token.rs
│   ├── lexer.rs / ast.rs / parser.rs
│   ├── resolver.rs / compiler.rs / chunk.rs / opcode.rs
│   ├── value.rs / object.rs / vm.rs
│   ├── environment.rs     # tree-walking oracle
│   └── repl.rs
└── tests/
    ├── integration/{main,lexer,parser,compiler,vm}.rs
    └── fixtures/*.vesper + *.out
```

## License

Licensed under either of MIT or Apache-2.0, at your option.
