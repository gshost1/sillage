//! Real-code fixtures: extraction, `validate()`, span and re-parse invariants, scoping.

mod common;

use sillage_core::{Lang, SimplifiedTree};
use sillage_parse::{ExtractedMethod, extract_methods, parse_method, parse_snippet};

pub const FIXTURES: &[(Lang, &str)] = &[
    (Lang::Java, include_str!("fixtures/Sample.java")),
    (Lang::Python, include_str!("fixtures/sample.py")),
    (Lang::JavaScript, include_str!("fixtures/sample.js")),
    (Lang::TypeScript, include_str!("fixtures/sample.ts")),
    (Lang::Tsx, include_str!("fixtures/sample.tsx")),
    (Lang::Go, include_str!("fixtures/sample.go")),
    (Lang::Rust, include_str!("fixtures/sample.rs")),
];

fn methods(lang: Lang) -> (&'static str, Vec<ExtractedMethod>) {
    let src = FIXTURES.iter().find(|(l, _)| *l == lang).unwrap().1;
    (src, extract_methods(lang, src))
}

fn method<'a>(ms: &'a [ExtractedMethod], name: &str) -> &'a ExtractedMethod {
    ms.iter()
        .find(|m| &*m.name == name)
        .unwrap_or_else(|| panic!("method {name} not extracted"))
}

/// VarIds of every token with this text.
fn vars(t: &SimplifiedTree, text: &str) -> Vec<Option<u32>> {
    common::tokens(t)
        .into_iter()
        .filter(|(s, _)| s == text)
        .map(|(_, v)| v)
        .collect()
}

/// Method trees are body-only: a name that only occurs in the signature (e.g. an
/// unused parameter or a default value) is legitimately absent.
fn signature_only(t: &SimplifiedTree, text: &str) -> bool {
    let body = t.nodes.first().map(|n| &t.source[n.span.start as usize..n.span.end as usize]);
    vars(t, text).is_empty() && body.is_none_or(|b| !b.contains(text))
}

fn assert_local(t: &SimplifiedTree, text: &str) -> u32 {
    if signature_only(t, text) {
        return u32::MAX;
    }
    let vs = vars(t, text);
    assert!(!vs.is_empty(), "{text} not found:\n{}", common::dump(t));
    let first = vs.iter().flatten().next().copied();
    let Some(id) = first else {
        panic!("{text} should be local:\n{}", common::dump(t));
    };
    assert!(
        vs.iter().flatten().all(|v| *v == id),
        "{text} has several ids {vs:?}:\n{}",
        common::dump(t)
    );
    id
}

fn assert_global(t: &SimplifiedTree, text: &str) {
    if signature_only(t, text) {
        return;
    }
    let vs = vars(t, text);
    assert!(!vs.is_empty(), "{text} not found:\n{}", common::dump(t));
    assert!(vs.iter().all(Option::is_none), "{text} should not be a var {vs:?}:\n{}", common::dump(t));
}

#[test]
fn fixtures_extract_validate_and_reparse() {
    for &(lang, src) in FIXTURES {
        let ms = extract_methods(lang, src);
        assert!(ms.len() >= 4, "{lang}: only {} methods", ms.len());
        for m in &ms {
            let text = &src[m.text_span.start as usize..m.text_span.end as usize];
            assert!(m.body_span.end as usize <= text.len() && m.body_span.start < m.body_span.end);
            let line = src[..m.text_span.start as usize].matches('\n').count() as u32 + 1;
            assert_eq!(m.line_start, line, "{lang} {}", m.name);
            assert_eq!(&*m.tree.source, text);
            m.tree.validate().unwrap_or_else(|e| panic!("{lang} {}: {e}", m.name));
            common::check_spans(&m.tree);
            let body = &text[m.body_span.start as usize..m.body_span.end as usize];
            let body_has_tokens = body.chars().any(char::is_alphanumeric);
            assert!(
                !body_has_tokens || (!m.tree.is_empty() && !m.tree.leaves.is_empty()),
                "{lang} {}",
                m.name
            );
            // Body-only tree: every node lies inside the body span.
            for n in &m.tree.nodes {
                assert!(n.span.start >= m.body_span.start && n.span.end <= m.body_span.end, "{lang} {}", m.name);
            }
            let again = parse_method(lang, text, m.body_span).unwrap();
            assert_eq!(again, m.tree, "{lang} {}: parse_method differs", m.name);
            if std::env::var_os("SILLAGE_DUMP").is_some() {
                println!("== {lang} {} (line {})\n{}", m.name, m.line_start, common::dump(&m.tree));
            }
        }
        // The whole file as a snippet also lowers cleanly.
        let t = parse_snippet(lang, src).unwrap();
        t.validate().unwrap();
        common::check_spans(&t);
    }
}

#[test]
fn extracted_names() {
    let expect: &[(Lang, &[&str])] = &[
        (Lang::Java, &["Sample", "run", "sum", "readAll", "mapAll", "describe", "withBody"]),
        (Lang::Python, &["top_level", "__init__", "name", "scan", "fetch", "bump"]),
        (Lang::JavaScript, &["merge", "square", "add", "constructor", "fetchJson", "ids", "secret", "hoisting", "onClick", "onKey"]),
        (Lang::TypeScript, &["each", "parse", "drain", "pick", "describe", "helper"]),
        (Lang::Tsx, &["List", "Footer", "increment", "render"]),
        (Lang::Go, &["NewStore", "Add", "Keys", "Sum", "Describe"]),
        (Lang::Rust, &["new", "dist", "fmt", "describe", "area", "histogram", "nested", "helper"]),
    ];
    for (lang, names) in expect {
        let (_, ms) = methods(*lang);
        let got: Vec<&str> = ms.iter().map(|m| &*m.name).collect();
        for n in *names {
            assert!(got.contains(n), "{lang}: {n} missing from {got:?}");
        }
        if *lang == Lang::Java {
            assert!(!got.contains(&"noBody"), "abstract method has no body");
        }
    }
}

#[test]
fn java_scoping() {
    let (_, ms) = methods(Lang::Java);
    let t = &method(&ms, "sum").tree;
    let values = assert_local(t, "values");
    let total = assert_local(t, "total");
    let i = assert_local(t, "i");
    assert!(values != total && total != i);
    assert_global(t, "LIMIT");
    assert_global(t, "length");

    let t = &method(&ms, "readAll").tree;
    assert_local(t, "reader");
    assert_local(t, "line");
    assert_local(t, "e");
    assert_local(t, "path");
    assert_local(t, "count"); // field: free names are locals (Decision 8, revised)
    assert_global(t, "readLine");
    assert_global(t, "append");

    let t = &method(&ms, "mapAll").tree;
    assert_local(t, "x");
    assert_local(t, "fn");
    assert_local(t, "out");
    assert_local(t, "items"); // field: free names are locals (Decision 8, revised)
    assert_global(t, "toString");
    assert_global(t, "println");

    let t = &method(&ms, "describe").tree;
    assert_local(t, "s");
    assert_local(t, "k");
    assert_local(t, "text");
    assert_global(t, "outer"); // a label
}

#[test]
fn python_scoping() {
    let (_, ms) = methods(Lang::Python);
    let t = &method(&ms, "top_level").tree;
    for v in ["a", "b", "c", "args", "kwargs", "total", "x", "y"] {
        assert_local(t, v);
    }
    assert_global(t, "zip");

    let t = &method(&ms, "scan").tree;
    for v in ["self", "seen", "fh", "sink", "line", "n", "err", "squares", "lookup", "key", "item", "v", "k"] {
        assert!(signature_only(t, v) || vars(t, v).iter().any(Option::is_some), "{v} local:\n{}", common::dump(t));
    }
    assert_global(t, "COUNTER");
    assert_global(t, "os");
    assert_global(t, "path");
    assert_global(t, "print");

    let t = &method(&ms, "fetch").tree;
    let count = assert_local(t, "count");
    let _ = count;
    for v in ["retries", "client", "result", "a", "b", "rest", "data"] {
        assert_local(t, v);
    }
    assert_local(t, "bump");
    assert_global(t, "get");
}

#[test]
fn javascript_scoping() {
    let (_, ms) = methods(Lang::JavaScript);
    let t = &method(&ms, "merge").tree;
    for v in ["target", "sources", "src", "key"] {
        assert_local(t, v);
    }
    assert_global(t, "Object");
    assert_global(t, "hasOwnProperty");

    let t = &method(&ms, "hoisting").tree;
    for v in ["flag", "later", "a", "renamed", "others", "re", "items", "x", "idx", "v"] {
        assert_local(t, v);
    }
    assert_global(t, "square");
    assert_global(t, "b");

    let t = &method(&ms, "constructor").tree;
    for v in ["url", "retries", "first", "third"] {
        assert!(signature_only(t, v) || vars(t, v).iter().any(Option::is_some), "{v} local:\n{}", common::dump(t));
    }
    assert_global(t, "DEFAULTS");
}

#[test]
fn typescript_scoping() {
    let (_, ms) = methods(Lang::TypeScript);
    let t = &method(&ms, "parse").tree;
    for v in ["input", "opts", "out", "retries", "part", "n", "total"] {
        assert_local(t, v);
    }
    assert_global(t, "Number");
    let t = &method(&ms, "pick").tree;
    for v in ["obj", "keys", "result", "k"] {
        assert_local(t, v);
    }
    let (_, ms) = methods(Lang::Tsx);
    let t = &method(&ms, "List").tree;
    for v in ["title", "items", "onSelect", "selected", "setSelected", "item"] {
        assert_local(t, v);
    }
    assert_global(t, "useState");
}

#[test]
fn go_scoping() {
    let (_, ms) = methods(Lang::Go);
    let t = &method(&ms, "Add").tree;
    for v in ["s", "key", "delta", "v", "ok"] {
        assert_local(t, v);
    }
    assert_global(t, "errors");
    let t = &method(&ms, "Sum").tree;
    for v in ["values", "total", "i", "v", "square", "x"] {
        assert_local(t, v);
    }
    let t = &method(&ms, "Describe").tree;
    for v in ["x", "ch", "t", "msg", "limit", "raw", "r", "f", "i", "n"] {
        assert_local(t, v);
    }
    assert_global(t, "fmt");
}

#[test]
fn rust_scoping() {
    let (_, ms) = methods(Lang::Rust);
    let t = &method(&ms, "dist").tree;
    for v in ["other", "dx", "dy"] {
        assert_local(t, v);
    }
    let t = &method(&ms, "histogram").tree;
    for v in ["words", "counts", "w", "total", "word", "n", "raw", "bytes", "c", "i"] {
        assert!(vars(t, v).iter().any(Option::is_some), "{v} local:\n{}", common::dump(t));
    }
    assert_global(t, "HashMap");
    let t = &method(&ms, "nested").tree;
    for v in ["v", "items", "add", "acc", "a", "b", "helper"] {
        assert!(vars(t, v).iter().any(Option::is_some), "{v} local:\n{}", common::dump(t));
    }
    assert_global(t, "Some");
    assert_global(t, "None");
}
