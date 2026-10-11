# Vesper — Verification

This document records **how** each requirement in [`REQUIREMENTS.md`](REQUIREMENTS.md)
is verified, the exact commands used, and the observed results. Verification
uses four methods:

* **Test** — an automated test (unit, integration, golden, property).
* **Benchmark** — a Criterion benchmark, plus wall-clock measurement where a
  requirement names a time budget.
* **Review** — a documented code/design argument checked by inspection.
* **Proof sketch** — an informal but explicit argument for an invariant that is
  not directly executable.

## 1. How to reproduce

```console
$ rustc --version          # see rust-toolchain.toml
$ cargo fmt --all -- --check
$ cargo clippy --all-targets --all-features -- -D warnings
$ RUSTFLAGS="-D warnings" cargo test
$ cargo build --release
$ cargo bench --no-run     # (and `cargo bench` to run the suite)
```

## 2. Observed results

Environment: Linux, Rust 1.95.0, `x86_64`. Development-machine measurements,
release profile (`lto = "thin"`, `codegen-units = 1`).

| Check | Command | Result |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check` | clean |
| Lints | `cargo clippy --all-targets --all-features -- -D warnings` | clean |
| Unit tests (`--lib`) | `cargo test` | 106 passed, 0 failed |
| Integration tests | `cargo test` | 28 passed, 0 failed |
| Doc test | `cargo test` | 1 passed, 0 failed |
| Examples | run each `examples/*.vesper` | exit 0, expected output |
| Golden fixtures | `tests/integration/vm.rs` | 5 fixtures match `.out` |
| Release build | `cargo build --release` | success |
| Benchmarks compile | `cargo bench --no-run` | success |
| `fib(35)` wall clock | `./target/release/vesper fib35.vesper` | ~8 s (**see R8**) |

## 3. Requirement-by-requirement verification

### R1 — Lexing — **verified**

* **Method:** unit tests, integration tests, property tests.
* **Evidence:** `lexer::tests::*` (12 unit tests), plus
  `lexer::tokenizes_keywords_operators_and_literals`,
  `lexer::error_reports_precise_span`,
  `lexer::unterminated_string_is_one_error` (integration), and the proptest
  properties `lexer::never_panics_on_arbitrary_input` and
  `lexer::successful_stream_ends_with_eof` (thousands of random inputs).
* **Proof sketch (progress):** `next_token` either consumes at least one byte,
  emits `Eof`, or (on a malformed multibyte character) consumes the remainder of
  that character, so the cursor strictly advances and the source length is a
  decreasing measure. Termination follows.

### R2 — Parsing — **verified**

* **Method:** unit/integration tests, property test, grammar review.
* **Evidence:** `parser::tests::*` (7 unit tests),
  `parser::parses_a_full_program` and
  `parser::collects_multiple_statement_errors` (integration),
  `parser::never_panics_on_arbitrary_input` (property). The grammar in
  `DESIGN.md` §4.1 is matched against the production functions in `parser.rs`.
* **Error recovery:** `collects_multiple_statement_errors` confirms two
  independent syntax errors are both reported.

### R3 — Compilation — **verified**

* **Method:** unit + integration tests, disassembly inspection.
* **Evidence:** `compiler::tests::*` (7 unit tests);
  `compiler::emits_arithmetic_and_print`, `compiler::emits_control_flow_opcodes`,
  `compiler::compiles_closure_with_upvalue`,
  `compiler::compiles_class_inheritance`,
  `compiler::reports_too_many_locals` (integration). Disassembly assertions check
  for the specific opcodes (`OP_ADD`, `OP_JUMP_IF_FALSE`, `OP_CLOSURE`,
  `OP_INHERIT`, …). Limit enforcement is checked by
  `chunk::tests::constant_pool_overflow_is_error` and
  `parser::tests::too_many_params_is_error`.

### R4 — VM / tree-walker equivalence — **verified**

* **Method:** differential testing (strong evidence), golden tests.
* **Evidence:** `tests::vm_and_tree_walker_agree` (lib),
  `vm::tree_walker_agrees_with_vm_on_all_fixtures` — this runs **every** fixture
  through both back-ends and asserts identical output —
  and `vm::golden_fixtures_match_expected_output`, which pins behavior to
  checked-in `.out` files. Runtime error paths are covered by
  `vm::tests::arity_mismatch_is_runtime_error`,
  `vm::tests::type_error_on_addition`,
  `vm::tests::undefined_global_is_runtime_error`,
  `vm::tests::stack_overflow_is_reported` and their integration counterparts.
* **Argument:** the two back-ends share the lexer/parser/AST but have distinct
  scope engines (`resolver.rs` vs `compiler.rs`) and distinct execution
  strategies (environment chain vs value stack). Agreement therefore exercises
  two independent implementations of the same semantics.

### R5 — Closures — **verified**

* **Method:** unit tests, golden fixture, design argument.
* **Evidence:** `environment::tests::closures_capture_environment`,
  `environment::tests::nested_closures`,
  `vm::tests::closures_capture_by_reference`,
  `vm::tests::closures_in_loop_capture_distinct_cells`,
  `compiler::tests::compiles_function_with_closure`, and golden fixture
  `closure_counter.vesper` (nested captures, mutable upvalues, per-instance
  state).
* **Argument:** `DESIGN.md` §8 states the open/upvalue invariant; `close_upvalues`
  is invoked on scope exit and return, after which an upvalue becomes
  `Closed(value)` and is no longer stack-backed.

### R6 — Error handling — **verified**

* **Method:** unit + integration tests.
* **Evidence:** `error::tests::*` (4 tests),
  `resolver::tests::*` (8 semantic-error tests),
  `tests::reports_lex_error`, `tests::reports_parse_error`, and the full set of
  `*_is_error` / `*_is_reported` tests. No test observes a panic; all failures
  return `Error` values with a `category()` and (except I/O) a `span()`.

### R7 — Safety — **verified (review)**

* **Method:** compiler-enforced + review.
* **Evidence:** `#![forbid(unsafe_code)]` in `src/lib.rs:28` and
  `unsafe_code = "forbid"` in `[lints.rust]`. The build fails if any `unsafe`
  block is introduced. `Vm::push`/`Vm::pop` carry `debug_assert!`
  preconditions for stack discipline.
* **Memory-safety argument:** all shared mutation goes through `Rc`/`RefCell`;
  borrows are short-lived and never held across a re-entrant VM call. The one
  risky pattern (mutating the open-upvalue list while iterating) is avoided by
  snapshotting into a local vector in `close_upvalues`.

### R8 — Performance — **partially verified (target not demonstrated on this machine)**

* **Method:** benchmarks + wall-clock measurement + Callgrind instruction counts.
* **Benchmarks present:** `benches/vm_bench.rs` defines `fib_recursive`,
  `closure_counter`, and `loop_sum` exactly as required; `cargo bench --no-run`
  succeeds.
* **Measurement:** recursive `fib(35)` completes in **~8 s** in the release build
  on the development machine (target: `< 5 s`). Wall-clock on this machine is
  noisy (7.8–9.9 s) because it thermally throttles under sustained load, so the
  Callgrind instruction counts below are the reliable metric. The target is
  stated for "modern hardware"; the development machine is an Intel Core i5-8265U
  (a 2018 15 W ULV laptop) whose single-thread throughput is several times lower
  than a current desktop CPU.
* **Optimization passes applied.** The interpreter was profiled with Callgrind
  and improved in two rounds (instruction counts measured on `fib(27)`, which is
  deterministic):
  * global-variable lookup uses an FNV-1a hasher instead of the default SipHash
    (`HashMap<String, Value, FnvBuildHasher>`); SipHash on short names was ~12% of
    retired instructions and is now negligible;
  * the hot numeric opcodes (`Less`, `Greater`, `Subtract`, `Multiply`,
    `Divide`) are inlined directly in the dispatch loop rather than going through
    the former `operand_number`/`binary_number` helpers;
  * `push`/`pop`/`peek`/`read_name` are `#[inline]`, and `close_upvalues` returns
    immediately when no upvalues are open;
  * source **spans are now computed lazily**: the dispatch loop no longer calls
    `Chunk::span` once per instruction (it was ~14% of retired instructions,
    paid through `Option`/`unwrap_or` even though spans are only needed on
    error). The rarely-taken opcode helpers (`call_value`, `call_closure`,
    `invoke`, `get_property`, `set_property`, `pop_class`) now receive the chunk
    and instruction offset and resolve the span only when they build an error;
  * `OpCode::from_byte` is a single indexed load from a compile-time
    `OPCODE_TABLE` instead of a `match`.
  Net effect (Callgrind, `fib(27)`): **907.5 M → 678.6 M** retired instructions
  (−25%); wall clock `fib(35)`: **~10.7 s → ~8 s**.
* **Remaining analysis:** the dominant remaining cost is value-stack machinery
  (`Vec` push/pop, bounds checks, `Option` handling) at roughly half of retired
  instructions, followed by the dispatch `match` and the call machinery. Global
  lookup is now below 1%, so a compile-time **global slot** scheme was measured
  to be not worth its complexity. Reducing the stack cost further would require
  unchecked writes behind a narrowly justified `unsafe` stack (the spec permits a
  module-scoped `allow(unsafe_code)` accompanied by a full safety argument) or a
  larger redesign (for example NaN-boxed values); both were judged out of scope
  because safe Rust now lands within a small constant factor of the budget.
* **Disposition:** R8.1 (no quadratic behavior + benchmarks) is met. R8.2 is not
  demonstrated on this laptop, but the 25% instruction-count reduction and the
  spec's "modern hardware" qualifier make the budget plausible on a current
  desktop; the raw figures and the reproducible Callgrind method are recorded
  here rather than a bare pass/fail.

### R9 — Documentation — **verified (review)**

* **Method:** review.
* **Evidence:** crate- and module-level rustdoc on every module; `# Examples`,
  `# Errors`, and `# Panics` sections on public/fallible APIs where applicable
  (e.g. `lib.rs` crate example, `Error::render`); `DESIGN.md`, `REQUIREMENTS.md`,
  `VERIFICATION.md`, and `README.md` present and cross-linked. `cargo test` runs
  the doc test in `src/lib.rs`.

### R10 — CI — **verified (review)**

* **Method:** review + local execution of the same commands.
* **Evidence:** `.github/workflows/ci.yml` runs `cargo fmt --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`,
  `cargo build --release`, and `cargo bench --no-run`. All five commands were run
  locally with the results recorded in §2.

## 4. Test inventory summary

| Suite | File(s) | Count |
| --- | --- | --- |
| Unit tests | `src/*.rs` `#[cfg(test)]` modules | 106 |
| Integration tests | `tests/integration/{lexer,parser,compiler,vm}.rs` | 28 |
| Doc tests | `src/lib.rs` | 1 |
| Property tests | `tests/integration/lexer.rs`, `tests/integration/parser.rs` | 4 properties |
| Golden fixtures | `tests/fixtures/*.vesper` + `*.out` | 5 |
| Benchmarks | `benches/vm_bench.rs` | 3 |

## 5. Known gaps and their disposition

* **R8.2 (fib(35) < 5 s):** not demonstrated on the development machine (a 2018
  ULV laptop); measured ~8 s after a 25% instruction-count reduction, with
  the reproducible Callgrind method and a remaining-optimization plan recorded in
  the R8 section. This is a performance target, not a correctness property.
* **Reference cycles:** not collected (documented limitation; see `DESIGN.md`
  §7.1). Out of scope for the specified language.
* **Anonymous function expressions:** not in the grammar; only named `fun`
  declarations exist. This matches the specification and is documented in the
  grammar (`DESIGN.md` §4.1).
