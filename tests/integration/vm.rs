//! VM integration tests: golden fixture output, tree-walker agreement, and
//! runtime error paths.

use std::fs;
use std::path::{Path, PathBuf};
use vesper::{run_source, run_tree_source};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every `.vesper` fixture under `tests/fixtures/` paired with a `.out` golden
/// file.
fn fixtures() -> Vec<(PathBuf, String, String)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(fixtures_dir()).expect("fixtures dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("vesper") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("fixture source");
        let golden_path = path.with_extension("out");
        let expected = fs::read_to_string(&golden_path)
            .unwrap_or_else(|_| panic!("missing golden file {}", golden_path.display()));
        out.push((path, source, expected));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn golden_fixtures_match_expected_output() {
    let all = fixtures();
    assert!(all.len() >= 5, "expected at least 5 fixtures");
    for (path, source, expected) in all {
        let actual =
            run_source(&source, false).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(actual, expected, "fixture {}", path.display());
    }
}

#[test]
fn tree_walker_agrees_with_vm_on_all_fixtures() {
    for (path, source, _) in fixtures() {
        let vm = run_source(&source, false).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let tree = run_tree_source(&source)
            .unwrap_or_else(|e| panic!("tree-walker {}: {e}", path.display()));
        assert_eq!(vm, tree, "back-ends disagree on {}", path.display());
    }
}

#[test]
fn undefined_variable_is_runtime_error() {
    let err = run_source("print missing;", false).unwrap_err();
    assert_eq!(err.category(), "runtime");
    assert!(err.to_string().contains("missing"));
}

#[test]
fn type_error_is_reported() {
    let err = run_source("print 1 + true;", false).unwrap_err();
    assert_eq!(err.category(), "runtime");
}

#[test]
fn arity_mismatch_is_reported() {
    let err = run_source("fun f(a) { return a; } f();", false).unwrap_err();
    assert_eq!(err.category(), "runtime");
    assert!(err.to_string().contains("argument"));
}

#[test]
fn stack_overflow_is_reported_not_a_panic() {
    let err = run_source("fun f() { return f(); } f();", false).unwrap_err();
    assert_eq!(err.category(), "runtime");
    assert!(err.to_string().contains("overflow"));
}

#[test]
fn trace_mode_appends_trace_lines() {
    let output = run_source("print 1;", true).unwrap();
    assert!(output.contains("trace"));
}
