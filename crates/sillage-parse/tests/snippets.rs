//! Decision 8: free value identifiers in snippets are locals; callees, members,
//! types, builtins and capitalised names are not. Full methods use the same rule
//! (Decision 8, revised) so fields read alike in queries and in the corpus.

mod common;

use sillage_core::{Lang, SimplifiedTree};
use sillage_parse::{parse_method, parse_snippet};

fn var(t: &SimplifiedTree, name: &str) -> Option<u32> {
    let toks = common::tokens(t);
    let vs: Vec<Option<u32>> = toks.iter().filter(|(s, _)| s == name).map(|(_, v)| *v).collect();
    assert!(!vs.is_empty(), "{name} missing:\n{}", common::dump(t));
    assert!(vs.iter().all(|v| *v == vs[0]), "{name}: {vs:?}\n{}", common::dump(t));
    vs[0]
}

fn snippet(lang: Lang, s: &str) -> SimplifiedTree {
    let t = parse_snippet(lang, s).unwrap();
    t.validate().unwrap();
    common::check_spans(&t);
    t
}

#[test]
fn free_identifiers_per_language() {
    // (lang, snippet, locals, non-locals)
    let cases: &[(Lang, &str, &[&str], &[&str])] = &[
        (Lang::Java, "int m = Math.max(a, b.size());\nlist.add(m);", &["m", "a", "b", "list"], &["Math", "max", "size", "add"]),
        (Lang::Java, "view.getContext()", &["view"], &["getContext"]),
        (Lang::Python, "result = helper(data, key=k)\nprint(len(result))", &["result", "data", "k"], &["helper", "key", "print", "len"]),
        (Lang::JavaScript, "const y = compute(x.value, {z});\nconsole.log(y);", &["y", "x", "z"], &["compute", "value", "console", "log"]),
        (Lang::TypeScript, "const n: number = items.length + offset;", &["n", "items", "offset"], &["length"]),
        (Lang::Tsx, "return <Item value={count} onClick={handle} />;", &["count", "handle"], &["Item"]),
        (Lang::Go, "n := len(buf)\nfmt.Println(n, total)", &["n", "buf", "total"], &["len", "fmt", "Println"]),
        (Lang::Rust, "let v = compute(&input);\nout.push(v.len());\nNone", &["v", "input", "out"], &["compute", "push", "len", "None"]),
    ];
    for (lang, s, locals, globals) in cases {
        let t = snippet(*lang, s);
        for l in *locals {
            assert!(var(&t, l).is_some(), "{lang}: {l} should be local\n{}", common::dump(&t));
        }
        for g in *globals {
            assert_eq!(var(&t, g), None, "{lang}: {g} should not be local\n{}", common::dump(&t));
        }
    }
}

#[test]
fn methods_treat_free_names_as_locals() {
    let t = parse_method(
        Lang::Java,
        "void f(int a) { int b = a + limit; log(b); }",
        Default::default(),
    )
    .unwrap();
    t.validate().unwrap();
    assert!(var(&t, "a").is_some() && var(&t, "b").is_some());
    assert!(var(&t, "limit").is_some(), "free value name (e.g. a field) is a local");
    assert_eq!(var(&t, "log"), None, "callee");

    let t = parse_method(
        Lang::Python,
        "def f(a):\n    global g\n    g = a\n    return a + other\n",
        Default::default(),
    )
    .unwrap();
    t.validate().unwrap();
    assert!(var(&t, "a").is_some());
    assert_eq!(var(&t, "g"), None, "declared `global`");
    assert!(var(&t, "other").is_some());
}

#[test]
fn snippet_kinds_parse_cleanly() {
    let cases: &[(Lang, &str, &str)] = &[
        // A whole method as a snippet.
        (Lang::Java, "public int twice(int x) { return x * 2; }", "x"),
        // A bare expression.
        (Lang::Java, "a.b(c) + d", "d"),
        (Lang::JavaScript, "class A { m(p) { return p; } }", "p"),
        (Lang::Python, "def g(p):\n    return p\n", "p"),
        (Lang::Go, "func (s *S) M(p int) int { return p }", "p"),
        (Lang::Rust, "fn m(&self, p: u8) -> u8 { p }", "p"),
    ];
    for (lang, s, local) in cases {
        let t = snippet(*lang, s);
        assert!(var(&t, local).is_some(), "{lang}: {local}\n{}", common::dump(&t));
        assert!(!common::labels(&t).iter().any(|l| l.contains("__")));
        assert!(common::tokens(&t).iter().all(|(s, _)| !s.starts_with("__")));
    }
}

/// Label of the root, with every hole (internal node labelled "") shown as `#`.
fn root_label(t: &SimplifiedTree) -> String {
    t.label(0).to_string()
}

fn has_hole(t: &SimplifiedTree) -> bool {
    common::labels(t).iter().any(|l| l.is_empty())
}

/// Stage 3: a dangling operator gets a featureless hole, so the expression reads
/// like the complete code (`#=#`, `#&&#`, `#.#`), with no placeholder token.
#[test]
fn dangling_operators_get_holes() {
    let cases: &[(Lang, &str, &str)] = &[
        (Lang::Java, "x =", "#=#"),
        (Lang::Java, "int y = a &&", "int#;"),
        (Lang::Java, "foo.", "#.#"),
        (Lang::JavaScript, "const z = a ||", "const#"),
        (Lang::Python, "total = a +", "#=#"),
        (Lang::Go, "v :=", "#:=#"),
    ];
    for (lang, s, root) in cases {
        let t = snippet(*lang, s);
        assert!(has_hole(&t), "{lang} {s:?}:\n{}", common::dump(&t));
        if !root.is_empty() {
            assert_eq!(&root_label(&t), root, "{lang} {s:?}:\n{}", common::dump(&t));
        }
        assert!(common::tokens(&t).iter().all(|(s, _)| !s.contains("sillage_hole")));
    }
    // `a &&` is a two-operand `&&`.
    let t = snippet(Lang::Java, "boolean b = a &&");
    assert!(common::labels(&t).iter().any(|l| l == "#&&#"), "{}", common::dump(&t));
}

/// A trailing `,` is left alone (no hole): the call's arity is unknown.
#[test]
fn trailing_comma_gets_no_hole() {
    let t = snippet(Lang::Java, "foo(a, b,");
    assert!(!has_hole(&t), "{}", common::dump(&t));
    // Complete postfix operators and Rust's `?` are not dangling.
    for (lang, s) in [(Lang::Java, "i++"), (Lang::Rust, "let v = f()?"), (Lang::Python, "if x:\n    y = 1")] {
        assert!(!has_hole(&snippet(lang, s)), "{lang} {s:?}");
    }
}

/// A leading-dot fluent-chain line whose receiver was not sampled reads
/// `#.#` with a hole receiver, like `x.build()` in the method; a chain continued
/// from the previous line keeps its real receiver.
#[test]
fn leading_dot_lines_get_hole_receivers() {
    let t = snippet(Lang::Java, "        .build();");
    assert_eq!(root_label(&t), "#;", "{}", common::dump(&t));
    assert!(has_hole(&t));
    assert!(common::labels(&t).iter().any(|l| l == "#.#"));
    // Continued chain: no hole.
    let t = snippet(Lang::Java, "Foo f = new Foo.Builder()\n    .setX(1)\n    .build();");
    assert!(!has_hole(&t), "{}", common::dump(&t));
    // After a complete statement: hole.
    let t = snippet(Lang::Java, "a();\n    .build();");
    assert!(has_hole(&t), "{}", common::dump(&t));
    let t = snippet(Lang::TypeScript, "await x;\n  .then((r) => r.json());");
    assert!(has_hole(&t), "{}", common::dump(&t));
}

/// `this(…)` / `super(…)` parse error-free inside a constructor wrapper, with
/// the same shape as in a corpus constructor body.
#[test]
fn explicit_constructor_calls_parse_in_constructor_wrapper() {
    let t = snippet(Lang::Java, "this(uri, 0);");
    assert_eq!(root_label(&t), "this(#,#);", "{}", common::dump(&t));
    let t = snippet(Lang::Java, "super(src.type);\nname = src.name;");
    assert!(common::labels(&t).iter().any(|l| l == "super(#);"), "{}", common::dump(&t));
    let m = "class A { A(B src) { super(src.type);\nname = src.name; } }";
    let ex = &sillage_parse::extract_methods(Lang::Java, m)[0];
    let body = parse_method(Lang::Java, &m[ex.text_span.start as usize..ex.text_span.end as usize], ex.body_span).unwrap();
    assert!(common::labels(&body).iter().any(|l| l == "super(#);"), "{}", common::dump(&body));
}
