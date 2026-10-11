//! Criterion benchmarks for the Vesper bytecode VM.
//!
//! Three workloads are measured, matching the specification:
//!
//! * `fib_recursive` — function-call and recursion throughput.
//! * `closure_counter` — closure allocation and upvalue read/write throughput.
//! * `loop_sum` — tight numeric loop throughput.
//!
//! Each benchmark compiles its source once, outside the measured closure, so the
//! reported time is VM execution only.

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::rc::Rc;
use vesper::compile_source;
use vesper::object::Function;
use vesper::vm::Vm;

const FIB: &str = "fun fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } fib(20);";

const CLOSURE_COUNTER: &str = "\
fun make_counter() { var n = 0; fun inc() { n = n + 1; return n; } return inc; } \
var c = make_counter(); \
for (var i = 0; i < 1000; i = i + 1) { c(); }";

const LOOP_SUM: &str = "\
var total = 0; \
for (var i = 0; i < 100000; i = i + 1) { total = total + i; } \
print total;";

fn compile(source: &str) -> Rc<Function> {
    compile_source(source).expect("compile")
}

/// Runs a precompiled script on a fresh VM.
fn run(function: &Rc<Function>) -> String {
    let mut vm = Vm::new();
    vm.interpret(Rc::clone(function)).expect("run")
}

fn benches(c: &mut Criterion) {
    let fib = compile(FIB);
    let counter = compile(CLOSURE_COUNTER);
    let loop_sum = compile(LOOP_SUM);

    c.bench_function("fib_recursive", |b| b.iter(|| black_box(run(&fib))));
    c.bench_function("closure_counter", |b| {
        b.iter(|| black_box(run(&counter)));
    });
    c.bench_function("loop_sum", |b| b.iter(|| black_box(run(&loop_sum))));
}

criterion_group!(benches_group, benches);
criterion_main!(benches_group);
