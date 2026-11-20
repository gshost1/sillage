//! Stage 1 (agent engine): import extraction, import relevance and test detection.

use sillage_core::Lang;
use sillage_parse::{extract_imports, extract_methods, is_test_method, is_test_path, relevant_imports, relevant_imports_excluding};

fn imports(lang: Lang, src: &str) -> Vec<String> {
    extract_imports(lang, src).into_iter().map(String::from).collect()
}

fn boxed(v: &[&str]) -> Vec<Box<str>> {
    v.iter().map(|s| Box::<str>::from(*s)).collect()
}

#[test]
fn extract_imports_java() {
    let src = "package a.b;\nimport java.util.List;\nimport static org.junit.Assert.assertEquals;\nimport java.io.*;\nimport java.util.List;\nclass A { void f() {} }";
    assert_eq!(
        imports(Lang::Java, src),
        ["import java.util.List;", "import static org.junit.Assert.assertEquals;", "import java.io.*;"]
    );
}

#[test]
fn extract_imports_python() {
    let src = "import os, sys as s\nfrom x.y import (\n    a,\n    b as c,\n)\nfrom . import z\n\ndef f():\n    import json\n    return json.dumps(1)\n";
    assert_eq!(
        imports(Lang::Python, src),
        ["import os, sys as s", "from x.y import (a, b as c,)", "from . import z", "import json"]
    );
}

#[test]
fn extract_imports_javascript() {
    let src = "import React, { useState } from 'react'\nimport * as fs from \"fs\";\nimport './x.css';\nconst path = require('path');\nfunction f() { const q = require('q'); }\n";
    assert_eq!(
        imports(Lang::JavaScript, src),
        [
            "import React, { useState } from 'react';",
            "import * as fs from \"fs\";",
            "import './x.css';",
            "const path = require('path');"
        ]
    );
}

#[test]
fn extract_imports_typescript_and_tsx() {
    let ts = "import type { A } from './a';\nimport { type B, C } from './b';\nimport x = require('y');\nexport function f(a: A): B { return C(a); }\n";
    assert_eq!(
        imports(Lang::TypeScript, ts),
        ["import type { A } from './a';", "import { type B, C } from './b';", "import x = require('y');"]
    );
    let tsx = "import React from 'react';\nimport { Button } from './Button';\nexport const App = () => <Button />;\n";
    assert_eq!(imports(Lang::Tsx, tsx), ["import React from 'react';", "import { Button } from './Button';"]);
}

#[test]
fn extract_imports_go() {
    let src = "package p\nimport \"fmt\"\nimport (\n\th \"net/http\"\n\t_ \"embed\"\n\t\"strings\"\n)\nfunc f() {}\n";
    assert_eq!(
        imports(Lang::Go, src),
        ["import \"fmt\"", "import h \"net/http\"", "import _ \"embed\"", "import \"strings\""]
    );
}

#[test]
fn extract_imports_rust() {
    let src = "use std::io::{self, Read};\npub(crate) use crate::a::B;\nextern crate foo;\n#[cfg(test)]\nmod tests {\n    use super::*;\n    use std::io::{self, Read};\n}\n";
    assert_eq!(
        imports(Lang::Rust, src),
        ["use std::io::{self, Read};", "use crate::a::B;", "extern crate foo;", "use super::*;"]
    );
}

#[test]
fn relevant_imports_keeps_only_referenced_names() {
    // Java: specific imports by simple name; wildcard only for unresolved types.
    let java = boxed(&["import java.util.List;", "import java.util.Map;", "import static org.junit.Assert.assertEquals;", "import java.io.*;"]);
    assert_eq!(relevant_imports(Lang::Java, &java, "List<String> xs = foo.Map();"), ["import java.util.List;"]);
    assert_eq!(
        relevant_imports(Lang::Java, &java, "assertEquals(1, x);\nreader.close(); // Map"),
        ["import static org.junit.Assert.assertEquals;"]
    );
    assert_eq!(
        relevant_imports(Lang::Java, &java, "BufferedReader r = new BufferedReader(in);"),
        ["import java.io.*;"]
    );

    // Python: narrowed from-imports, module bindings, string contents ignored.
    let py = boxed(&["import os, sys as s", "from x.y import (a, b as c,)", "import json"]);
    assert_eq!(
        relevant_imports(Lang::Python, &py, "if c(a):\n    print(\"json os\")\n    s.exit(1)"),
        ["import sys as s", "from x.y import a, b as c"]
    );
    assert_eq!(relevant_imports(Lang::Python, &py, "x = obj.json"), Vec::<String>::new());

    // JS/TS: default, namespace and named specifiers narrowed; side effects dropped.
    let js = boxed(&["import React, { useState, useEffect } from 'react';", "import * as fs from \"fs\";", "import './x.css';", "const path = require('path');"]);
    assert_eq!(
        relevant_imports(Lang::JavaScript, &js, "const [v, setV] = useState(0);\nfs.readFileSync(path.join(a, b));"),
        ["import { useState } from 'react';", "import * as fs from \"fs\";", "const path = require('path');"]
    );
    let ts = boxed(&["import type { A, B } from './a';"]);
    assert_eq!(relevant_imports(Lang::TypeScript, &ts, "let x: B = make();"), ["import type { B } from './a';"]);

    // Go: package names from paths and aliases; blank imports never.
    let go = boxed(&["import \"fmt\"", "import h \"net/http\"", "import _ \"embed\"", "import \"gopkg.in/yaml.v2\""]);
    assert_eq!(
        relevant_imports(Lang::Go, &go, "if err != nil {\n\treturn fmt.Errorf(\"http: %w\", err)\n}"),
        ["import \"fmt\""]
    );
    assert_eq!(relevant_imports(Lang::Go, &go, "h.Get(u)\nyaml.Unmarshal(b, &v)"), ["import h \"net/http\"", "import \"gopkg.in/yaml.v2\""]);

    // Rust: use lists narrowed, `self` binds the module, `::` members are not names.
    let rs = boxed(&["use std::io::{self, Read, Write};", "use anyhow::bail;", "use std::fmt::Error;", "use super::*;"]);
    assert_eq!(
        relevant_imports(Lang::Rust, &rs, "let n = r.read(&mut buf)?;\nif n == 0 { bail!(\"eof\"); }\nreturn Err(io::Error::other(\"x\"));"),
        ["use std::io;", "use anyhow::bail;"]
    );
    assert_eq!(relevant_imports(Lang::Rust, &rs, "fn g<R: Read>(r: R) {}"), ["use std::io::Read;"]);
    assert_eq!(relevant_imports(Lang::Rust, &rs, "let c = Config::default();"), ["use super::*;"]);
}

#[test]
fn relevant_imports_skips_what_the_caller_already_has() {
    let rs = boxed(&["use std::io::{self, Read};", "use anyhow::bail;"]);
    let existing = vec!["use anyhow::bail;".to_string(), "use std::io::Read as _;".to_string()];
    assert_eq!(
        relevant_imports_excluding(Lang::Rust, &rs, "bail!(\"x\"); io::stdin();", &existing),
        ["use std::io;"]
    );
    let py = boxed(&["from x import a, b"]);
    assert!(relevant_imports_excluding(Lang::Python, &py, "a(b)", &["from x import a, b".into()]).is_empty());
}

#[test]
fn test_paths_by_language() {
    let yes = [
        (Lang::Rust, "tests/integration.rs"),
        (Lang::Rust, "src/foo_tests.rs"),
        (Lang::Rust, "src/test_support.rs"),
        (Lang::Rust, "benches/foo_test.rs"),
        (Lang::Rust, "src/foo/tests.rs"),
        (Lang::Go, "pkg/server_test.go"),
        (Lang::Java, "src/test/java/a/Foo.java"),
        (Lang::Java, "src/main/java/a/FooTest.java"),
        (Lang::Java, "a/FooIT.java"),
        (Lang::Python, "tests/test_api.py"),
        (Lang::Python, "pkg/test_api.py"),
        (Lang::Python, "pkg/api_test.py"),
        (Lang::Python, "conftest.py"),
        (Lang::JavaScript, "src/a.test.js"),
        (Lang::TypeScript, "src/a.spec.ts"),
        (Lang::Tsx, "src/__tests__/App.tsx"),
        (Lang::Tsx, "src\\components\\Button.test.tsx"),
    ];
    for (lang, p) in yes {
        assert!(is_test_path(lang, p), "{lang} {p}");
    }
    let no = [
        (Lang::Rust, "src/contest.rs"),
        (Lang::Rust, "src/attestation/mod.rs"),
        // Singular `*_test.rs` under `src/` is a domain module in Rust.
        (Lang::Rust, "/repo/src/foo_test.rs"),
        (Lang::Rust, "crates/tabref-page/src/act/hit_test.rs"),
        (Lang::Rust, "crates/linux/src/tree/hit_test/live_world.rs"),
        (Lang::Rust, "src/self_test.rs"),
        (Lang::Rust, "src/testing.rs"),
        (Lang::Rust, "src/test_.rs"),
        (Lang::Go, "pkg/testing_utils.go"),
        (Lang::Java, "src/main/java/a/Testimony.java"),
        (Lang::Java, "src/main/java/a/Latest.java"),
        (Lang::Python, "pkg/latest.py"),
        (Lang::Python, "pkg/contest.py"),
        (Lang::JavaScript, "src/attest.js"),
        (Lang::TypeScript, "src/protest/index.ts"),
    ];
    for (lang, p) in no {
        assert!(!is_test_path(lang, p), "{lang} {p}");
    }
}

#[test]
fn test_methods_by_markers() {
    assert!(is_test_method(Lang::Rust, "src/a.rs", "it_works", "#[test]\nfn it_works() {}"));
    assert!(is_test_method(Lang::Rust, "src/a.rs", "t", "#[tokio::test]\nasync fn t() {}"));
    assert!(!is_test_method(Lang::Rust, "src/a.rs", "f", "#[inline]\nfn f() { let test = 1; }"));
    assert!(is_test_method(Lang::Go, "pkg/a.go", "TestX", "func TestX(t *testing.T) {}"));
    assert!(is_test_method(Lang::Go, "pkg/a_test.go", "helper", "func helper() {}"));
    assert!(!is_test_method(Lang::Go, "pkg/a.go", "TestimonyCount", "func TestimonyCount() int { return 1 }"));
    assert!(is_test_method(Lang::Java, "src/main/A.java", "t", "@Test\npublic void t() { assertEquals(1, 1); }"));
    assert!(!is_test_method(Lang::Java, "src/main/A.java", "f", "public void f() { String s = \"@Test\"; }"));
    assert!(is_test_method(Lang::Python, "pkg/a.py", "test_add", "def test_add():\n    assert add(1, 2) == 3"));
    assert!(!is_test_method(Lang::Python, "pkg/a.py", "add", "def add(a, b):\n    return a + b"));
    assert!(is_test_method(Lang::JavaScript, "src/a.js", "suite", "function suite() { describe('x', () => {}); }"));
    assert!(!is_test_method(Lang::JavaScript, "src/a.js", "check", "function check(s) { return /x/.test(s); }"));
}

#[test]
fn extraction_marks_out_of_text_test_context() {
    let rs = "fn prod() { 1; }\n#[test]\nfn unit() { assert!(true); }\n#[cfg(test)]\nmod tests {\n    fn helper() -> u8 { 2 }\n}\n";
    let ctx: Vec<(String, bool)> = extract_methods(Lang::Rust, rs).into_iter().map(|m| (m.name.to_string(), m.test_context)).collect();
    assert_eq!(ctx, [("prod".into(), false), ("unit".into(), true), ("helper".into(), true)]);

    let py = "class TestApi(unittest.TestCase):\n    def setUp(self):\n        self.x = 1\n\ndef prod(a):\n    return a\n\n@pytest.fixture\ndef client():\n    return 1\n";
    let ctx: Vec<(String, bool)> = extract_methods(Lang::Python, py).into_iter().map(|m| (m.name.to_string(), m.test_context)).collect();
    assert_eq!(ctx, [("setUp".into(), true), ("prod".into(), false), ("client".into(), true)]);

    let js = "function prod(a) { return a; }\ndescribe('x', () => { function helper() { return 1; } });\n";
    let ctx: Vec<(String, bool)> = extract_methods(Lang::JavaScript, js).into_iter().map(|m| (m.name.to_string(), m.test_context)).collect();
    assert_eq!(ctx, [("prod".into(), false), ("helper".into(), true)]);

    let java = "class FooTest { void helper() { int x = 1; } }\nclass Foo { void prod() { int y = 2; } }\n";
    let ctx: Vec<(String, bool)> = extract_methods(Lang::Java, java).into_iter().map(|m| (m.name.to_string(), m.test_context)).collect();
    assert_eq!(ctx, [("helper".into(), true), ("prod".into(), false)]);
}

/// `is_test` as the corpus computes it: extraction context or path/name/text markers.
fn rust_is_test(path: &str, src: &str) -> Vec<(String, bool)> {
    extract_methods(Lang::Rust, src)
        .into_iter()
        .map(|m| {
            let text = &src[m.text_span.start as usize..m.text_span.end as usize];
            let t = m.test_context || is_test_method(Lang::Rust, path, &m.name, text);
            (m.name.to_string(), t)
        })
        .collect()
}

#[test]
fn rust_singular_test_stems_are_prod_unless_marked() {
    let hit = "pub(crate) fn confirm_hit(x: i32, y: i32) -> bool {\n    x >= 0 && y >= 0\n}\n";
    assert_eq!(rust_is_test("crates/tabref-page/src/act/hit_test.rs", hit), [("confirm_hit".into(), false)]);
    assert_eq!(rust_is_test("gshost1/tabref/crates/tabref-page/src/act/hit_test.rs", hit), [("confirm_hit".into(), false)]);
    let live = "pub fn live_world() -> u8 { 1 }\n";
    assert_eq!(rust_is_test("crates/linux/src/tree/hit_test/live_world.rs", live), [("live_world".into(), false)]);

    let helper = "fn make_folder() -> u8 { 1 }\n";
    assert_eq!(rust_is_test("crates/tabref/src/commands/download_folder_tests.rs", helper), [("make_folder".into(), true)]);
    assert_eq!(rust_is_test("crates/tabref/tests/live_downloads_daemon.rs", helper), [("make_folder".into(), true)]);

    let marked = "#[test]\nfn it_works() { assert!(true); }\n";
    assert_eq!(rust_is_test("src/foo_test.rs", marked), [("it_works".into(), true)]);
    let cfg = "#[cfg(test)]\nmod tests {\n    fn helper() -> u8 { 2 }\n}\n";
    assert_eq!(rust_is_test("src/foo_test.rs", cfg), [("helper".into(), true)]);
    let inline = "pub fn prod() -> u8 { 1 }\n#[cfg(test)]\nmod tests {\n    fn helper() {}\n    #[test]\n    fn it_works() {}\n}\n";
    assert_eq!(
        rust_is_test("src/foo.rs", inline),
        [("prod".into(), false), ("helper".into(), true), ("it_works".into(), true)]
    );
}
