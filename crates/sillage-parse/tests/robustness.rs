//! Snippet robustness: windows of lines cut from real methods, plus odd inputs,
//! never panic and always satisfy `validate()` with spans inside the snippet.

mod common;

use proptest::prelude::*;
use sillage_core::Lang;
use sillage_parse::{extract_methods, parse_snippet};

const FIXTURES: &[(Lang, &str)] = &[
    (Lang::Java, include_str!("fixtures/Sample.java")),
    (Lang::Python, include_str!("fixtures/sample.py")),
    (Lang::JavaScript, include_str!("fixtures/sample.js")),
    (Lang::TypeScript, include_str!("fixtures/sample.ts")),
    (Lang::Tsx, include_str!("fixtures/sample.tsx")),
    (Lang::Go, include_str!("fixtures/sample.go")),
    (Lang::Rust, include_str!("fixtures/sample.rs")),
];

fn check(lang: Lang, snippet: &str) {
    let t = parse_snippet(lang, snippet).unwrap_or_else(|e| panic!("{lang}: {e}\n{snippet}"));
    t.validate()
        .unwrap_or_else(|e| panic!("{lang}: {e}\n---\n{snippet}\n---\n{}", common::dump(&t)));
    assert_eq!(&*t.source, snippet);
    common::check_spans(&t);
}

/// Multi-line method texts from all fixtures.
fn method_texts() -> Vec<(Lang, String)> {
    let mut out = Vec::new();
    for &(lang, src) in FIXTURES {
        for m in extract_methods(lang, src) {
            let text = &src[m.text_span.start as usize..m.text_span.end as usize];
            if text.lines().count() >= 5 {
                out.push((lang, text.to_string()));
            }
        }
    }
    out
}

fn window(text: &str, start: usize, len: usize) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = start.min(lines.len().saturating_sub(1));
    lines[start..(start + len).min(lines.len())].concat()
}

#[test]
fn first_five_lines_of_every_method() {
    let ms = method_texts();
    assert!(ms.len() >= 20, "only {} multi-line methods", ms.len());
    let langs: std::collections::BTreeSet<Lang> = ms.iter().map(|(l, _)| *l).collect();
    assert_eq!(langs.len(), 7);
    for (lang, text) in &ms {
        check(*lang, &window(text, 0, 5));
    }
}

#[test]
fn every_five_line_window_of_every_method() {
    for (lang, text) in method_texts() {
        let n = text.lines().count();
        for start in 0..n {
            check(lang, &window(&text, start, 5));
        }
    }
}

#[test]
fn fragment_free_variables_and_python_indentation() {
    // A fragment from the middle of a Python method keeps its locals.
    let snippet = "        for line in fh:\n            seen.add(line.strip())\n";
    let t = parse_snippet(Lang::Python, snippet).unwrap();
    t.validate().unwrap();
    common::check_spans(&t);
    let toks = common::tokens(&t);
    let var = |n: &str| toks.iter().find(|(t, _)| t == n).unwrap().1;
    assert!(var("line").is_some() && var("fh").is_some() && var("seen").is_some());
    assert_eq!(var("add"), None);
    assert_eq!(var("strip"), None);
    let lines: Vec<_> = toks.iter().filter(|(t, _)| t == "line").map(|(_, v)| *v).collect();
    assert!(lines.iter().all(|v| *v == lines[0]));
}

#[test]
fn odd_inputs() {
    let deep = format!("{}x{}", "(".repeat(3000), ")".repeat(3000));
    let long_chain = format!("x = {};", vec!["a"; 5000].join(" + "));
    let cases: Vec<String> = vec![
        String::new(),
        "   \n\t ".into(),
        "}".into(),
        "{{{{".into(),
        ")))) ;; ((".into(),
        "\"unterminated".into(),
        "'''unterminated\nstill".into(),
        "/* open comment".into(),
        "// just a comment".into(),
        "x".into(),
        "return".into(),
        "héllo wörld ✓ λ → 漢字".into(),
        "\u{0}\u{1}binary\u{7f}".into(),
        deep,
        long_chain,
    ];
    for &(lang, _) in FIXTURES {
        for c in &cases {
            check(lang, c);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn random_windows(idx in 0usize..1000, start in 0usize..200, len in 1usize..8) {
        let ms = method_texts();
        let (lang, text) = &ms[idx % ms.len()];
        let n = text.lines().count();
        check(*lang, &window(text, start % n, len));
    }

    #[test]
    fn random_byte_ranges(idx in 0usize..1000, a in 0usize..4000, b in 0usize..400) {
        let ms = method_texts();
        let (lang, text) = &ms[idx % ms.len()];
        let mut s = a % text.len();
        while !text.is_char_boundary(s) { s -= 1; }
        let mut e = (s + b).min(text.len());
        while !text.is_char_boundary(e) { e -= 1; }
        check(*lang, &text[s..e]);
    }

    #[test]
    fn arbitrary_text(lang_idx in 0usize..7, s in "\\PC{0,80}") {
        check(Lang::ALL[lang_idx], &s);
    }
}
