# Vesper — Requirements

This document states Vesper's numbered requirements and traces each to the
tests, benchmarks, review, or proof sketch that satisfies it. Verification
methods are described in [`VERIFICATION.md`](VERIFICATION.md).

Legend: **U** = unit test (`#[cfg(test)]` in a module), **I** = integration test
(`tests/integration/`), **G** = golden fixture test, **P** = property test
(proptest), **B** = Criterion benchmark, **R** = code review / documented
argument.

## Functional requirements

### R1 — Lexing

**R1.1** The lexer tokenizes every language construct: keywords, identifiers,
numbers (with fractions and scientific notation), strings (with escapes),
punctuators, operators, comments, and whitespace.

*Tests:* `lexer::tests::keywords_are_recognized`,
`lexer::tests::punctuators_and_operators`,
`lexer::tests::numbers_with_fraction_and_exponent`,
`lexer::tests::string_literals_with_escapes`, `lexer::tests::comments_are_skipped`,
`lexer::tests::identifier_boundaries`,
`lexer::tokenizes_keywords_operators_and_literals` (I),
`lexer::numbers_carry_their_literal_value` (I),
`lexer::string_escapes_are_decoded` (I).

**R1.2** The lexer reports precise spans for errors and never reads past the
end of input.

*Tests:* `lexer::tests::spans_are_correct`,
`lexer::tests::unexpected_character_is_error`,
`lexer::tests::invalid_escape_is_error`,
`lexer::tests::unterminated_string_is_error`,
`lexer::tests::unterminated_block_comment_is_error`,
`lexer::tests::multiline_errors_recover_and_report`,
`lexer::error_reports_precise_span` (I),
`lexer::unterminated_string_is_one_error` (I).

**R1.3** The lexer always terminates and never panics on arbitrary input.

*Tests:* `lexer::tests::never_panics_and_terminates`,
`lexer::never_panics_on_arbitrary_input` (P),
`lexer::successful_stream_ends_with_eof` (P).

### R2 — Parsing

**R2.1** The parser accepts the full grammar (see `DESIGN.md` §4.1).

*Tests:* `parser::tests::parses_var_and_print`,
`parser::tests::parses_function_decl`,
`parser::tests::parses_class_with_super`,
`parser::tests::parses_for_natively`,
`parser::tests::assignment_is_right_associative`,
`parser::parses_a_full_program` (I), `parser::function_with_closure_body_parses` (I).

**R2.2** The parser rejects invalid programs with useful messages and recovers
to report multiple errors.

*Tests:* `parser::tests::missing_semicolon_is_error`,
`parser::tests::assignment_to_non_var_is_error`,
`parser::tests::too_many_params_is_error`,
`parser::tests::reports_multiple_errors`,
`parser::collects_multiple_statement_errors` (I),
`parser::rejects_trailing_garbage` (I).

**R2.3** The parser never panics on arbitrary input.

*Tests:* `parser::never_panics_on_arbitrary_input` (P).

### R3 — Compilation

**R3.1** The compiler emits bytecode equivalent to the source semantics,
including control flow with patched jumps, functions, closures with upvalue
descriptors, and classes/inheritance/methods.

*Tests:* `compiler::tests::compiles_arithmetic`,
`compiler::tests::continue_in_for_compiles`,
`compiler::tests::compiles_function_with_closure`,
`compiler::tests::compiles_class_with_methods`,
`compiler::tests::compiles_inheritance`,
`compiler::emits_arithmetic_and_print` (I),
`compiler::emits_control_flow_opcodes` (I),
`compiler::compiles_closure_with_upvalue` (I),
`compiler::compiles_class_inheritance` (I).

**R3.2** Compile-time limits are enforced with typed errors: locals,
parameters, upvalues, constant pool.

*Tests:* `compiler::tests::too_many_locals_is_error`,
`compiler::reports_too_many_locals` (I),
`parser::tests::too_many_params_is_error`,
`chunk::tests::constant_pool_overflow_is_error`.

**R3.3** Constant pools are de-duplicated.

*Tests:* `chunk::tests::constants_are_deduplicated`,
`compiler::constant_pool_is_deduplicated` (I).

### R4 — VM/tree-walker equivalence

**R4.1** The bytecode VM has the same observable (`print`) behavior as the
tree-walking interpreter.

*Tests:* `tests::vm_and_tree_walker_agree` (lib),
`vm::tree_walker_agrees_with_vm_on_all_fixtures` (I over all fixtures),
`vm::golden_fixtures_match_expected_output` (G).

**R4.2** The VM implements control flow, functions, classes, and closures with
correct semantics.

*Tests:* `vm::tests::arithmetic`, `vm::tests::strings_concatenate`,
`vm::tests::control_flow_and_loops`, `vm::tests::break_and_continue`,
`vm::tests::recursion`, `vm::tests::classes_and_methods`,
`vm::tests::inheritance_and_super`, `vm::tests::initializer_returns_instance`,
`vm::tests::closures_capture_by_reference`,
`vm::tests::closures_in_loop_capture_distinct_cells`,
`vm::tests::clock_is_callable`.

**R4.3** Runtime errors are reported (not panics): arity, type errors, undefined
globals, stack overflow.

*Tests:* `vm::tests::arity_mismatch_is_runtime_error`,
`vm::tests::type_error_on_addition`,
`vm::tests::undefined_global_is_runtime_error`,
`vm::tests::stack_overflow_is_reported`,
`vm::arity_mismatch_is_reported` (I), `vm::type_error_is_reported` (I),
`vm::undefined_variable_is_runtime_error` (I),
`vm::stack_overflow_is_reported_not_a_panic` (I).

### R5 — Closures

**R5.1** Closures capture the correct environment, including nested captures and
per-iteration capture in loops.

*Tests:* `environment::tests::closures_capture_environment`,
`environment::tests::nested_closures`,
`vm::tests::closures_capture_by_reference`,
`vm::tests::closures_in_loop_capture_distinct_cells`,
`compiler::tests::compiles_function_with_closure`,
golden fixture `closure_counter.vesper` (G).

*Argument:* `DESIGN.md` §8 states the open/closed-upvalue invariant; the
`close_upvalues` helper closes cells on scope exit and return.

### R6 — Error handling

**R6.1** Every error path is typed, carries a span where relevant, and is never
silently swallowed.

*Tests:* `error::tests::category_names`, `error::tests::io_has_no_span`,
`error::tests::render_includes_caret`,
`error::tests::render_handles_io_without_span`,
`tests::reports_lex_error`, `tests::reports_parse_error`,
`resolver::tests::*` (semantic errors), and the full set of `*_is_error` tests
across modules.

**R6.2** Resolution-time semantic errors are detected: redeclaration, read in own
initializer, top-level `return`, `this` outside a class, `super` without a
superclass, unresolved globals.

*Tests:* `resolver::tests::redeclaration_is_error`,
`resolver::tests::read_in_own_initializer_is_error`,
`resolver::tests::return_at_top_level_is_error`,
`resolver::tests::this_outside_class_is_error`,
`resolver::tests::super_without_superclass_is_error`,
`resolver::tests::globals_are_not_resolved_locally`,
`resolver::tests::valid_class_resolves_cleanly`.

### R7 — Safety

**R7.1** There is no undefined behavior; `unsafe` is confined to one audited,
hot-path module and every unchecked operation there documents its safety.

*Verification:* `unsafe_code = "deny"` at the crate root (`src/lib.rs`) and in
`[lints.rust]`; the only module that opts back in is `src/stack.rs`
(`#[allow(unsafe_code)]`), which states the `Stack` invariants and the safety
argument for each unchecked primitive. Every other module compiles under the
denial, so introducing `unsafe` elsewhere fails the build; **R**.

**R7.2** All critical stack/frame invariants are asserted in debug builds.

*Verification:* `debug_assert!` in `Vm::push`/`Vm::pop` and stack-height checks;
**R**.

### R8 — Performance

**R8.1** Benchmarks exist for recursive Fibonacci, closure-heavy code, and loop
throughput.

*Tests:* `benches/vm_bench.rs` — `fib_recursive`, `closure_counter`,
`loop_sum`; **B**.

**R8.2** `fib(35)` completes in reasonable time (target `< 5 s` in release on
modern hardware).

*Verification:* measured; see `VERIFICATION.md`. On the development machine
(a 2018 15 W ULV laptop) it runs in **~8.5 s**; the requirement is tracked as
partially verified (target not demonstrated on this hardware), with the
completed optimization passes and the remaining path recorded there. **B/R**.

### R9 — Documentation

**R9.1** Every public item has rustdoc; module docs explain architecture.

*Verification:* crate-level and module-level docs throughout; `missing_docs` is
`allow`ed only because doc-comment coverage is enforced by review; **R**.

**R9.2** Process artifacts exist: `DESIGN.md`, `REQUIREMENTS.md`,
`VERIFICATION.md`, and a README with architecture diagram and usage.

*Verification:* files present and cross-linked; **R**.

### R10 — CI

**R10.1** CI runs format, clippy (`-D warnings`), tests, and build on push.

*Verification:* `.github/workflows/ci.yml`; **R**. The same commands pass
locally (see `VERIFICATION.md`).

## Non-functional / process requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| N1 | `Cargo.lock` committed for reproducible builds | **R** |
| N2 | `rust-toolchain.toml` pins the toolchain | **R** |
| N3 | Clippy pedantic + nursery enabled via `[lints.clippy]` | **R** (CI `-D warnings`) |
| N4 | No magic numbers; limits are named constants | **R** (`MAX_PARAMS`, `MAX_LOCALS`, `MAX_UPVALUES`, `MAX_CONSTANTS`, `MAX_FRAMES`, `STACK_MAX`) |
| N5 | Linear-time hot paths; complexity documented | **R** (`DESIGN.md` §9.1) |
| N6 | License is MIT OR Apache-2.0 | **R** (`LICENSE`, `Cargo.toml`) |
