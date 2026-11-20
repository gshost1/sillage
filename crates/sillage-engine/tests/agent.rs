//! Stage 1 (agent engine): filtered search, variable alignment, idioms,
//! missing guards, test idioms and diff review on small real repositories.

use std::fs;
use std::path::Path;

use sillage_core::{Featurizer, Lang, MethodScope, Params};
use sillage_engine::{
    Confidence, Engine, GuardKind, GuardOptions, IdiomOptions, QueryFilter, TestIdiomOptions, TestLink, prune,
    render_aligned,
};
use sillage_index::{AromaFeaturizer, index_dir};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn engine(root: &Path, params: Params) -> Engine {
    let (parts, _) = index_dir(root, Lang::ALL, &AromaFeaturizer).unwrap();
    Engine::from_parts(parts, Box::new(AromaFeaturizer), params).with_root(root)
}

/// A Go "load one row" function with the repo's two guards: the query error
/// check and the deferred close.
fn loader(func: &str, typ: &str, table: &str, rows: &str, v: &str) -> String {
    format!(
        "func {func}(db *DB, id string) (*{typ}, error) {{
\t{rows}, err := db.Query(\"SELECT id, name FROM {table} WHERE id = ?\", id)
\tif err != nil {{
\t\treturn nil, err
\t}}
\tdefer {rows}.Close()
\t{v} := &{typ}{{}}
\tif err := {rows}.Scan(&{v}.ID, &{v}.Name); err != nil {{
\t\treturn nil, err
\t}}
\treturn {v}, nil
}}
"
    )
}

const DRAFT_MISSING: &str = "func LoadInvoice(db *DB, id string) (*Invoice, error) {
\tres, err := db.Query(\"SELECT id, name FROM invoices WHERE id = ?\", id)
\tinv := &Invoice{}
\tif err := res.Scan(&inv.ID, &inv.Name); err != nil {
\t\treturn nil, err
\t}
\treturn inv, nil
}
";

fn go_repo(root: &Path) {
    write(
        root,
        "store/users.go",
        &format!("package store\n\nimport (\n\t\"database/sql\"\n\t\"fmt\"\n)\n\n{}\n{}", loader("LoadUser", "User", "users", "row", "u"), loader("LoadAdmin", "Admin", "admins", "r", "a")),
    );
    write(root, "store/orders.go", &format!("package store\n\nimport \"strings\"\n\n{}", loader("LoadOrder", "Order", "orders", "rs", "o")));
    write(root, "store/items.go", &format!("package store\n\n{}", loader("LoadItem", "Item", "items", "q", "it")));
    // Test code that skips both guards: must not count as a peer for prod checks.
    write(
        root,
        "store/users_test.go",
        "package store\n\nimport \"testing\"\n\nfunc TestLoadUser(t *testing.T) {\n\tdb := openTestDB(t)\n\tu, err := LoadUser(db, \"1\")\n\tif err != nil {\n\t\tt.Fatalf(\"load: %v\", err)\n\t}\n\tif u.Name != \"ann\" {\n\t\tt.Errorf(\"name = %q\", u.Name)\n\t}\n}\n\nfunc loadRaw(db *DB, id string) (*User, error) {\n\trow, _ := db.Query(\"SELECT id, name FROM users WHERE id = ?\", id)\n\tu := &User{}\n\trow.Scan(&u.ID, &u.Name)\n\treturn u, nil\n}\n",
    );
}

#[test]
fn missing_guards_found_by_consensus_and_silent_on_clean_code() {
    let dir = tempfile::tempdir().unwrap();
    go_repo(dir.path());
    let e = engine(dir.path(), Params::repo_preset());

    let guards = e.check_missing_guards(Lang::Go, DRAFT_MISSING, &GuardOptions::default()).unwrap();
    let heads: Vec<(&str, GuardKind)> = guards.iter().map(|g| (g.head.as_str(), g.kind)).collect();
    assert!(heads.contains(&("if err != nil {", GuardKind::ErrorCheck)), "{heads:?}");
    assert!(heads.contains(&("defer res.Close()", GuardKind::Cleanup)), "aligned to the draft's `res`: {heads:?}");
    for g in &guards {
        assert_eq!(g.confidence, Confidence::High);
        assert!(g.cluster_size >= 3 && g.distinct_files >= 2, "{g:?}");
        assert!(g.provenance.iter().all(|c| !c.is_test && !c.path.ends_with("_test.go")), "prod scope: {g:?}");
        assert!(g.provenance.iter().all(|c| c.provenance == format!("{}:{}", c.path, c.line)));
    }
    let err = guards.iter().find(|g| g.kind == GuardKind::ErrorCheck && g.head == "if err != nil {").unwrap();
    assert_eq!(err.code, "if err != nil {\n\treturn nil, err\n}");
    // The cited line is the guard's own line in the peer's file.
    let c = &err.provenance[0];
    let src = fs::read_to_string(dir.path().join(&c.path)).unwrap();
    assert_eq!(src.lines().nth(c.line as usize - 1).unwrap().trim(), "if err != nil {");

    // The complete function (both guards present) gets nothing.
    let clean = loader("LoadInvoice", "Invoice", "invoices", "res", "inv");
    assert!(e.check_missing_guards(Lang::Go, &clean, &GuardOptions::default()).unwrap().is_empty());

    // Fewer supporting peers than required: silent.
    let strict = GuardOptions { min_support: 5, ..GuardOptions::default() };
    assert!(e.check_missing_guards(Lang::Go, DRAFT_MISSING, &strict).unwrap().is_empty());
    // Unrelated code: no peers, silent.
    assert!(e.check_missing_guards(Lang::Go, "x := strings.Repeat(\"a\", 3)\nfmt.Println(x)", &GuardOptions::default()).unwrap().is_empty());
}

#[test]
fn missing_guards_over_a_git_diff() {
    let dir = tempfile::tempdir().unwrap();
    go_repo(dir.path());
    write(dir.path(), "store/invoices.go", &format!("package store\n\n{DRAFT_MISSING}"));
    let e = engine(dir.path(), Params::repo_preset());
    let diff = format!(
        "diff --git a/store/invoices.go b/store/invoices.go\nnew file mode 100644\n--- /dev/null\n+++ b/store/invoices.go\n@@ -0,0 +1,{} @@\n{}",
        DRAFT_MISSING.lines().count() + 2,
        ["package store", ""].iter().copied().chain(DRAFT_MISSING.lines()).map(|l| format!("+{l}\n")).collect::<String>()
    );
    let found = e.check_missing_guards_in_diff(&diff, &GuardOptions::default()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hunk.path, "store/invoices.go");
    assert!(found[0].guards.iter().any(|g| g.head == "defer res.Close()"), "{:?}", found[0].guards);
    // The hunk's own (indexed) file is never its own peer.
    assert!(found[0].guards.iter().all(|g| g.provenance.iter().all(|c| c.path != "store/invoices.go")));
}

#[test]
fn idioms_are_aligned_cited_and_confident() {
    let dir = tempfile::tempdir().unwrap();
    go_repo(dir.path());
    let e = engine(dir.path(), Params::repo_preset());
    // The agent's half-written body: query and scan, no error check, no close.
    let draft = "res, err := db.Query(\"SELECT id, name FROM invoices WHERE id = ?\", id)\ninv := &Invoice{}\nif err := res.Scan(&inv.ID, &inv.Name); err != nil {\n\treturn nil, err\n}\nreturn inv, nil";
    let idioms = e.find_idioms(Lang::Go, draft, &IdiomOptions::default()).unwrap();
    assert!(!idioms.is_empty(), "a consensus cluster forms with the repo preset");
    let top = &idioms[0];
    assert_eq!(top.confidence, Confidence::High, "{top:?}");
    assert!(top.cluster_size >= 3 && top.distinct_files >= 2);
    assert_eq!(top.provenance.len(), top.cluster_size);
    assert!(top.added_code.contains("if err != nil {"), "{}", top.added_code);
    assert!(top.code.contains("defer res.Close()"), "renamed to the draft's variable: {}", top.code);
    assert!(top.code.contains("res, err := db.Query("), "{}", top.code);
    assert!(top.renames.iter().any(|r| r.to == "res"), "{:?}", top.renames);
    assert!(top.diff.starts_with("--- draft\n+++ recommendation\n") && top.diff.contains("+defer res.Close()"), "{}", top.diff);
    assert!(!top.compact.is_empty());
    // No added line references an import, so none is suggested.
    assert!(top.relevant_imports.is_empty(), "{:?}", top.relevant_imports);

    // The paper defaults are stricter: on this tiny repo they form no cluster.
    let paper = engine(dir.path(), Params::default());
    let strict = paper.find_idioms(Lang::Go, draft, &IdiomOptions::default()).unwrap();
    assert!(strict.len() <= idioms.len());
}

#[test]
fn relevant_imports_follow_added_lines() {
    let dir = tempfile::tempdir().unwrap();
    let f = |name: &str, v: &str| {
        format!("func {name}({v} []string) string {{\n\tif len({v}) == 0 {{\n\t\treturn \"\"\n\t}}\n\tout := strings.Join({v}, \",\")\n\tlog.Printf(\"joined %d\", len({v}))\n\treturn out\n}}\n")
    };
    write(dir.path(), "a/a.go", &format!("package a\n\nimport (\n\t\"log\"\n\t\"strings\"\n\t\"os\"\n)\n\n{}\n{}", f("JoinA", "xs"), f("JoinB", "ys")));
    write(dir.path(), "b/b.go", &format!("package b\n\nimport (\n\t\"log\"\n\t\"strings\"\n)\n\n{}", f("JoinC", "zs")));
    let e = engine(dir.path(), Params::repo_preset());
    let idioms = e.find_idioms(Lang::Go, "out := strings.Join(names, \",\")\nreturn out", &IdiomOptions::default()).unwrap();
    let top = idioms.first().expect("idiom");
    assert!(top.added_code.contains("log.Printf"), "{}", top.added_code);
    assert!(top.relevant_imports.contains(&"import \"log\"".to_string()), "{:?}", top.relevant_imports);
    assert!(!top.relevant_imports.iter().any(|i| i.contains("\"os\"")), "unused import: {:?}", top.relevant_imports);
    assert!(top.code.contains("if len(names) == 0"), "{}", top.code);
    // Already imported by the draft's file: not suggested again.
    let opts = IdiomOptions { existing_imports: vec!["import \"log\"".into()], ..IdiomOptions::default() };
    let again = e.find_idioms(Lang::Go, "out := strings.Join(names, \",\")\nreturn out", &opts).unwrap();
    assert!(again[0].relevant_imports.is_empty(), "{:?}", again[0].relevant_imports);
}

#[test]
fn variable_alignment_never_corrupts_strings_comments_or_fields() {
    let text = "fn scale(p: &Point, k: f64) -> Point {\n    let x = p.x * k; // x is scaled\n    let y = p.y * k;\n    log(\"x and y\", x);\n    Point { x, y }\n}";
    let body_start = text.find('{').unwrap() as u32;
    let rec = sillage_parse::parse_method(Lang::Rust, text, sillage_core::Span::new(body_start, text.len() as u32)).unwrap();
    let draft = "let a = p.x * k;\nlet b = p.y * k;";
    let q = sillage_parse::parse_snippet(Lang::Rust, draft).unwrap();
    let fq = AromaFeaturizer.featurize_query(&q).total();
    let keep = prune(&fq, &rec, &AromaFeaturizer.featurize(&rec)).keep;
    let al = render_aligned(draft, &q, &rec, &keep, &AromaFeaturizer);
    assert_eq!(
        al.code,
        "let a = p.x * k; // x is scaled\nlet b = p.y * k;\nlog(\"x and y\", a);\nPoint { x: a, y: b }"
    );
    let mut renames: Vec<(String, String)> = al.renames.iter().map(|r| (r.from.clone(), r.to.clone())).collect();
    renames.sort();
    assert_eq!(renames, [("x".into(), "a".into()), ("y".into(), "b".into())]);
    assert_eq!(al.added_code, "log(\"x and y\", a);\nPoint { x: a, y: b }");
    assert_eq!(
        al.diff,
        "--- draft\n+++ recommendation\n@@ -1,2 +1,4 @@\n let a = p.x * k;\n let b = p.y * k;\n+log(\"x and y\", a);\n+Point { x: a, y: b }\n"
    );

    // A rename that would collide with an existing name is dropped.
    let draft2 = "let y = p.x * k;";
    let q2 = sillage_parse::parse_snippet(Lang::Rust, draft2).unwrap();
    let keep2 = prune(&AromaFeaturizer.featurize_query(&q2).total(), &rec, &AromaFeaturizer.featurize(&rec)).keep;
    let al2 = render_aligned(draft2, &q2, &rec, &keep2, &AromaFeaturizer);
    assert!(al2.renames.is_empty(), "{:?}", al2.renames);
    assert!(al2.code.contains("let x = p.x * k;"), "{}", al2.code);
}

#[test]
fn filter_runs_before_the_eta1_cut() {
    let dir = tempfile::tempdir().unwrap();
    let body = |name: &str| format!("func {name}(xs []int) int {{\n\tt := 0\n\tfor _, x := range xs {{\n\t\tt += x\n\t}}\n\treturn t\n}}\n");
    // Two exact matches in generated code, one weaker match elsewhere.
    write(dir.path(), "repo/gen/a.go", &format!("package gen\n\n{}", body("SumA")));
    write(dir.path(), "repo/gen/b.go", &format!("package gen\n\n{}\n\nfunc other() {{}}\n", body("SumB").replace("t := 0", "t := 0\n\t_ = len(xs)")));
    write(dir.path(), "repo/src/c.go", "package src\n\nfunc SumC(xs []int) int {\n\ttotal := 0\n\tfor i := range xs {\n\t\ttotal += xs[i] * 2\n\t}\n\treturn total\n}\n");
    write(dir.path(), "repo/src/c_test.go", &format!("package src\n\n{}", body("sumForTest").replace("return t", "_ = xs\n\treturn t")));
    let params = Params { eta1: 2, ..Params::repo_preset() };
    let e = engine(dir.path(), params);
    let q = "t := 0\nfor _, x := range xs {\n\tt += x\n}";

    let all = e.find_similar(Lang::Go, q, 10, &QueryFilter::new()).unwrap();
    assert_eq!(all.len(), 2, "η1 = 2 caps the candidates");
    assert!(all.iter().all(|m| m.name != "SumC"));

    // Excluding the generated directory frees both slots for the rest.
    let f = QueryFilter::new().exclude_path("repo/gen/");
    let names: Vec<String> = e.find_similar(Lang::Go, q, 10, &f).unwrap().into_iter().map(|m| m.name).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"SumC".to_string()) && names.contains(&"sumForTest".to_string()), "{names:?}");

    // Prod scope drops the test helper too; absolute and ./ paths work.
    let abs = dir.path().join("repo/gen/a.go").to_string_lossy().to_string();
    let f = QueryFilter::prod().exclude_path(abs).exclude_path("./repo/gen/b.go");
    let m = e.find_similar(Lang::Go, q, 10, &f).unwrap();
    assert_eq!(m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["SumC"]);
    assert!(!m[0].is_test && m[0].provenance == "repo/src/c.go:3" && m[0].snippet.starts_with("func SumC"));
    assert!(m[0].score > 0.0 && m[0].score <= 1.0 && m[0].line_end == 9);

    // Test scope keeps only test code.
    let t = e.find_similar(Lang::Go, q, 10, &QueryFilter::test()).unwrap();
    assert!(!t.is_empty() && t.iter().all(|m| m.is_test));
    assert_eq!(t[0].name, "sumForTest");

    // Excluded ids never reach recommendations either.
    let out = e.recommend_filtered(Lang::Go, q, &QueryFilter::new().exclude_path("gen/")).unwrap();
    assert!(out.n2.iter().all(|r| !e.method(r.id).unwrap().rel_path().contains("/gen/")));
    assert_eq!(QueryFilter::new().with_scope(MethodScope::All), QueryFilter::default());
}

#[test]
fn test_idioms_follow_production_peers() {
    let dir = tempfile::tempdir().unwrap();
    go_repo(dir.path());
    write(
        dir.path(),
        "store/orders_test.go",
        "package store\n\nimport (\n\t\"testing\"\n\t\"strings\"\n)\n\nfunc TestLoadOrder(t *testing.T) {\n\tdb := openTestDB(t)\n\to, err := LoadOrder(db, \"7\")\n\tif err != nil {\n\t\tt.Fatalf(\"load: %v\", err)\n\t}\n\tif o.ID != \"7\" {\n\t\tt.Errorf(\"id = %q\", o.ID)\n\t}\n}\n",
    );
    let e = engine(dir.path(), Params::repo_preset());
    let ti = e.find_test_idioms(Lang::Go, DRAFT_MISSING, &TestIdiomOptions::default()).unwrap();
    assert_eq!(ti.draft_name.as_deref(), Some("LoadInvoice"));
    assert!(ti.peers.iter().any(|p| p.name == "LoadUser") && ti.peers.iter().all(|p| !p.is_test));
    let by_name = |n: &str| ti.tests.iter().find(|t| t.test.name == n);
    let user = by_name("TestLoadUser").expect("test of a peer");
    assert_eq!(user.via, TestLink::ReferencesPeer);
    assert_eq!(user.peer.as_ref().unwrap().name, "LoadUser");
    let skel = user.skeleton.as_deref().unwrap();
    assert!(skel.contains("LoadInvoice(db, \"1\")") && !skel.contains("LoadUser"), "{skel}");
    assert!(user.assertions.iter().any(|a| a.starts_with("t.Fatalf")), "{:?}", user.assertions);
    assert!(user.test.is_test && user.test.provenance.starts_with("store/users_test.go:"));
    assert!(by_name("TestLoadOrder").is_some());
    let order = by_name("TestLoadOrder").unwrap();
    assert!(order.relevant_imports.contains(&"import \"testing\"".to_string()), "{:?}", order.relevant_imports);
    assert!(!order.relevant_imports.iter().any(|i| i.contains("strings")), "{:?}", order.relevant_imports);
    assert!(ti.tests.iter().all(|t| t.test.is_test));
    assert!(ti.tests.windows(2).all(|w| w[0].score >= w[1].score));
}

