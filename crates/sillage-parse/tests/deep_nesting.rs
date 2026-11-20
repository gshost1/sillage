//! Regression (C1): pathological nesting must not overflow the call stack in
//! tree walks. Runs on a test thread (2 MiB stack by default).

use sillage_core::Lang;
use sillage_parse::{extract_imports, parse_snippet, shorthand_spans};

const DEPTH: usize = 15_000;

fn nested_object(depth: usize) -> String {
    // `{a:{a:…{a}…}}`: only the innermost `a` is shorthand.
    format!("{}{{a}}{}", "{a:".repeat(depth - 1), "}".repeat(depth - 1))
}

#[test]
fn deep_js_literal_shorthand_spans() {
    let src = format!("function f(a) {{ return {}; }}", nested_object(DEPTH));
    let spans = shorthand_spans(Lang::JavaScript, &src);
    assert_eq!(spans.len(), 1, "the innermost `a` is a shorthand property");
}

#[test]
fn deep_js_destructuring_imports() {
    // `const {a:{a:…{a}…}} = require('m')` drives the pattern walk.
    let pat = nested_object(DEPTH);
    let src = format!("const {pat} = require('m');\nfunction f() {{ return a; }}");
    let imps = extract_imports(Lang::JavaScript, &src);
    assert_eq!(imps.len(), 1);
}

#[test]
fn deep_rust_use_tree() {
    let depth = 5_000;
    let src = format!("use {}x{};\nfn f() {{ x(); }}", "a::{".repeat(depth), "}".repeat(depth));
    let imps = extract_imports(Lang::Rust, &src);
    assert_eq!(imps.len(), 1);
}

#[test]
fn deep_js_snippet_parses() {
    let src = format!("let a = 1;\nlet o = {};", nested_object(DEPTH));
    // Must return (Ok or Err) without overflowing.
    let _ = parse_snippet(Lang::JavaScript, &src);
}
