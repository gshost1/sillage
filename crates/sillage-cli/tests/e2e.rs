//! End-to-end tests over the real pipeline: `sillage-parse` → `sillage-index`
//! (corpus walk, featurize, CSR index, BM25, save/load) → `sillage-engine`
//! (Phases I-III) → `sillage-render`. No hand-built trees.

mod common;

use common::*;
use sillage_core::{Featurizer, Lang, Span};
use sillage_index::{AromaFeaturizer, feature_debug};
use sillage_render::{Style, layout, render};

fn leaf_of(tree: &sillage_core::SimplifiedTree, text: &str, nth: usize) -> usize {
    tree.leaves
        .iter()
        .enumerate()
        .filter(|(_, l)| tree.label(**l) == text)
        .nth(nth)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("leaf {text}#{nth} missing"))
}

/// Paper Fig. 4 / Table 2 on the *real* tree-sitter parse of Listing 4.
#[test]
fn listing4_real_parse_matches_table2() {
    let tree = sillage_parse::parse_snippet(Lang::Java, LISTING4).unwrap();
    tree.validate().unwrap();
    assert_eq!(tree.label(0), "if##");

    // view_30: the second `view` (loop condition).
    let view30 = leaf_of(&tree, "view", 1);
    assert_eq!(
        feature_debug(&tree, view30),
        [
            "#VAR",
            "(#VAR, 2, (#)#)",
            "(#VAR, 1, (#))",
            "(#VAR, 1, #.#)",
            "(ViewGroup, #VAR)",
            "(#VAR, getChildCount)",
            "((1, #instanceof#), (2, (#)#))",
            "((2, (#)#), (2, (#)#))",
        ]
    );
    // 0_20: Table 2 prints `#;#;#;`; Fig. 4's for-control node is `#;#;#`.
    let zero = leaf_of(&tree, "0", 0);
    assert_eq!(
        feature_debug(&tree, zero),
        ["0", "(0, 2, #=#)", "(0, 1, int#)", "(0, 1, #;#;#)", "(#VAR, 0)", "(0, #VAR)"]
    );
}

/// Declaration and expression initialisers give the same loop shape (`for(#)#`
/// over `#;#;#`), so `for (int i = 0; ...)` and `for (i = 0; ...)` share features.
#[test]
fn for_header_shape_is_uniform() {
    for (lang, a, b) in [
        (Lang::Java, "for (int i = 0; i < n; i++) { f(i); }", "for (i = 0; i < n; i++) { f(i); }"),
        (Lang::JavaScript, "for (let i = 0; i < n; i++) { f(i); }", "for (i = 0; i < n; i++) { f(i); }"),
        (Lang::TypeScript, "for (let i = 0; i < n; i++) { f(i); }", "for (i = 0; i < n; i++) { f(i); }"),
    ] {
        let ta = sillage_parse::parse_snippet(lang, a).unwrap();
        let tb = sillage_parse::parse_snippet(lang, b).unwrap();
        assert_eq!(ta.label(0), "for(#)#", "{lang}");
        assert_eq!(tb.label(0), "for(#)#", "{lang}");
        assert_eq!(ta.label(ta.nodes[0].children[2]), "#;#;#", "{lang}");
    }
    // `for (;;)` keeps its `;`s as loop keywords.
    let t = sillage_parse::parse_snippet(Lang::Java, "for (;;) { f(); }").unwrap();
    assert_eq!(t.label(0), "for(;;)#");
}

/// A query snippet and a corpus method body holding the same statements produce
/// the same features (snippet free names ≡ method parameters, wrappers invisible).
#[test]
fn snippet_and_method_body_features_agree() {
    let method = format!("void setup(View view) {{\n{LISTING4}}}");
    let open = method.find('{').unwrap() as u32;
    let body = Span::new(open, method.len() as u32);
    let mtree = sillage_parse::parse_method(Lang::Java, &method, body).unwrap();
    mtree.validate().unwrap();
    // Body-only tree: no signature token survives.
    assert_eq!(mtree.label(0), "{#}");
    for n in &mtree.nodes {
        assert!(n.span.start >= open, "signature token in body tree: {:?}", mtree.label(0));
    }
    let stree = sillage_parse::parse_snippet(Lang::Java, LISTING4).unwrap();
    let fm = AromaFeaturizer.featurize(&mtree).total();
    let fs = AromaFeaturizer.featurize(&stree).total();
    assert_eq!(fm, fs, "snippet vs method-body features differ");

    // Same for an extracted method (corpus path) of Listing 5: the parameter `view`
    // is a local in the body, and the method name/params are not in the tree.
    let ms = sillage_parse::extract_methods(Lang::Java, LISTING5);
    let m = ms.iter().find(|m| &*m.name == "setupUIToHideKeyBoardOnTouch").unwrap();
    let toks: Vec<_> = m
        .tree
        .leaves
        .iter()
        .map(|&l| match &m.tree.node(l).kind {
            sillage_core::NodeKind::Token { text, var } => (text.to_string(), var.is_some()),
            _ => unreachable!(),
        })
        .collect();
    assert!(toks.iter().filter(|(t, _)| t == "view").all(|(_, v)| *v));
    assert_eq!(toks.iter().filter(|(t, _)| t == "setupUIToHideKeyBoardOnTouch").count(), 1, "only the recursive call");
    assert!(!toks.iter().any(|(t, _)| t == "public"));
}

/// Listings 4-6 → Listing 7 through the whole real pipeline and every renderer.
#[test]
fn listing4_to_listing7_end_to_end() {
    let dir = scratch("l7");
    let corpus = dir.path().join("corpus");
    write_paper_corpus(&corpus);
    let engine = engine_for(&corpus, &[Lang::Java], &dir.path().join("l7.sil"));
    assert_eq!(engine.corpus.methods.len(), 8, "4 distractors + 2 setup methods + 2 onTouch");

    // Structural search: Listings 5 and 6 contain the whole query → top 2.
    let ranked = engine.search(Lang::Java, LISTING4, 10).unwrap();
    let top2: std::collections::BTreeSet<String> = ranked[..2].iter().map(|r| method_name(&engine, r.id)).collect();
    assert_eq!(top2, ["setupUI".to_string(), "setupUIToHideKeyBoardOnTouch".to_string()].into());
    assert!(ranked[0].score > 0.95 && ranked[1].score > 0.95);
    assert!(ranked[2].score < ranked[1].score);

    let out = engine.recommend(Lang::Java, LISTING4).unwrap();
    assert!(!out.recommendations.is_empty());
    let rec = &out.recommendations[0];
    let mut names: Vec<String> = rec.cluster.iter().map(|&id| method_name(&engine, id)).collect();
    names.sort();
    assert_eq!(names, ["setupUI", "setupUIToHideKeyBoardOnTouch"]);
    assert!(rec.l > 1.5 && rec.s > 0.9, "l={} s={}", rec.l, rec.s);
    assert_eq!(rec.query_match.len(), rec.tree.leaves.len());

    let plain = render(&rec.tree, &rec.query_match, Some(LISTING4), Style::Plain);
    let expected = "\
if (!(view instanceof EditText)) {
    view.setOnTouchListener(new View.OnTouchListener() {
        public boolean onTouch(View v, MotionEvent event) {
            // ...
            return false;
        }
    });
}
if (view instanceof ViewGroup) {
    for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
        View innerView = ((ViewGroup) view).getChildAt(i);
        setupUIToHideKeyBoardOnTouch(innerView);
    }
}
";
    assert_eq!(plain, expected, "Listing 7 mismatch:\n{plain}");
    assert!(!plain.contains("hideKeyBoard()") && !plain.contains("toggleSoftKeyBoard"));

    // Highlighting: the touch-listener block and the recursive call are extra;
    // the query's own lines are not.
    let lay = layout(&rec.tree, &rec.query_match, Some(LISTING4));
    let extra = |needle: &str| lay.lines.iter().find(|l| l.text.trim() == needle).map(|l| l.extra);
    for l in [
        "if (!(view instanceof EditText)) {",
        "view.setOnTouchListener(new View.OnTouchListener() {",
        "public boolean onTouch(View v, MotionEvent event) {",
        "return false;",
        "// ...",
        "setupUIToHideKeyBoardOnTouch(innerView);",
    ] {
        assert_eq!(extra(l), Some(true), "{l} should be extra");
    }
    for l in [
        "if (view instanceof ViewGroup) {",
        "for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {",
        "View innerView = ((ViewGroup) view).getChildAt(i);",
    ] {
        assert_eq!(extra(l), Some(false), "{l} is query code");
    }
    let ph: Vec<_> = lay.lines.iter().filter(|l| l.placeholder).collect();
    assert_eq!(ph.len(), 1);
    assert!(lay.diff.as_ref().is_some_and(|d| d.inserted >= 7 && d.deleted == 0));

    let ansi = render(&rec.tree, &rec.query_match, Some(LISTING4), Style::Ansi);
    assert!(ansi.contains("\x1b[1;32m+ if (!(view instanceof EditText)) {\x1b[0m"), "{ansi}");
    assert!(ansi.contains("\n  if (view instanceof ViewGroup) {\n"), "{ansi}");

    let html = render(&rec.tree, &rec.query_match, Some(LISTING4), Style::Html);
    assert!(html.starts_with("<pre class=\"sillage-rec\""), "{html}");
    assert!(html.contains("extra") && html.contains("placeholder") && html.contains("matched"));
    assert!(html.contains("((ViewGroup) view)") && html.contains("&amp;") == plain.contains('&'));
    assert!(!html.contains("<View"), "unescaped generic/tag text");

    let json: serde_json::Value =
        serde_json::from_str(&render(&rec.tree, &rec.query_match, Some(LISTING4), Style::Json)).unwrap();
    assert_eq!(json["lang"], "java");
    assert_eq!(json["code"].as_str().unwrap().to_string() + "\n", plain);
    let lines = json["lines"].as_array().unwrap();
    assert_eq!(lines.len(), plain.lines().count());
    assert!(lines.iter().any(|l| l["placeholder"] == true && l["extra"] == true));

    // Singleton clusters (§4.3.5) recommend a full method body, unpruned.
    let single = out.recommendations.iter().find(|r| r.cluster.len() == 1).expect("a singleton cluster");
    let body = render(&single.tree, &single.query_match, None, Style::Plain);
    assert!(!body.contains("// ..."), "singleton must not be pruned:\n{body}");
    assert!(!body.contains("public void"), "signature must not leak:\n{body}");
}

/// Persistence round-trip preserves recommendations exactly.
#[test]
fn save_load_preserves_results() {
    let dir = scratch("persist");
    let corpus = dir.path().join("corpus");
    write_paper_corpus(&corpus);
    let a = engine_for(&corpus, &[Lang::Java], &dir.path().join("a.sil"));
    let (corpus_mem, _) = sillage_index::build_corpus(&corpus, &[Lang::Java]).unwrap();
    let inv = sillage_index::InvertedIndex::build(&corpus_mem, &AromaFeaturizer);
    let bm25 = sillage_index::Bm25::build(&corpus_mem);
    let b = sillage_engine::Engine::new(corpus_mem, inv, bm25, Box::new(AromaFeaturizer), Default::default());
    let ra = a.recommend(Lang::Java, LISTING4).unwrap();
    let rb = b.recommend(Lang::Java, LISTING4).unwrap();
    assert_eq!(ra.recommendations.len(), rb.recommendations.len());
    for (x, y) in ra.recommendations.iter().zip(&rb.recommendations) {
        assert_eq!(x.cluster, y.cluster);
        assert_eq!(x.tree, y.tree);
        assert_eq!(x.query_match, y.query_match);
    }
    assert_eq!(
        a.bm25_search(Lang::Java, "getChildAt innerView", 3),
        b.bm25_search(Lang::Java, "getChildAt innerView", 3)
    );
}

/// Mini real-code corpora for every language: two functions that extend the
/// query the same way (plus one distinct line each) and an unrelated distractor.
struct LangCase {
    lang: Lang,
    ext: &'static str,
    a: &'static str,
    b: &'static str,
    distractor: &'static str,
    query: &'static str,
    /// Shared extension that the intersection must keep.
    keeps: &'static [&'static str],
    /// Per-function lines the intersection must drop.
    drops: &'static [&'static str],
}

const CASES: &[LangCase] = &[
    LangCase {
        lang: Lang::Java,
        ext: "java",
        a: r#"class A {
    String loadA(File file) throws IOException {
        BufferedReader reader = new BufferedReader(new FileReader(file));
        StringBuilder sb = new StringBuilder();
        String line;
        while ((line = reader.readLine()) != null) {
            sb.append(line);
            auditTrail(4242);
        }
        reader.close();
        return sb.toString();
    }
}
"#,
        b: r#"class B {
    String loadB(File input) throws IOException {
        BufferedReader br = new BufferedReader(new FileReader(input));
        StringBuilder out = new StringBuilder();
        String line;
        while ((line = br.readLine()) != null) {
            out.append(line);
            Metrics.bumpCounter("zzz-unique");
        }
        br.close();
        return out.toString();
    }
}
"#,
        distractor: r#"class C {
    int fib(int n) {
        if (n < 2) {
            return n;
        }
        return fib(n - 1) + fib(n - 2);
    }
}
"#,
        query: "BufferedReader reader = new BufferedReader(new FileReader(file));\nStringBuilder sb = new StringBuilder();\nString line;\n",
        keeps: &["while ((line = reader.readLine()) != null) {", "sb.append(line);", "reader.close();", "return sb.toString();"],
        drops: &["auditTrail", "bumpCounter"],
    },
    LangCase {
        lang: Lang::Python,
        ext: "py",
        a: r#"import json


def load_a(path):
    with open(path) as fh:
        data = json.load(fh)
    result = {}
    for key, value in data.items():
        result[key.lower()] = value
        audit_trail(4242)
    return result
"#,
        b: r#"import json


def load_b(filename):
    with open(filename) as f:
        payload = json.load(f)
    result = {}
    for key, value in payload.items():
        result[key.lower()] = value
        metrics.bump_counter("zzz-unique")
    return result
"#,
        distractor: r#"def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)
"#,
        query: "with open(path) as fh:\n    data = json.load(fh)\n",
        keeps: &["result = {}", "for key, value in data.items():", "result[key.lower()] = value", "return result"],
        drops: &["audit_trail", "bump_counter"],
    },
    LangCase {
        lang: Lang::JavaScript,
        ext: "js",
        a: r#"async function fetchA(url) {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}`);
  }
  const body = await res.json();
  auditTrail(4242);
  return body.items.map((x) => x.id);
}
"#,
        b: r#"async function fetchB(endpoint) {
  const response = await fetch(endpoint);
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  const body = await response.json();
  metrics.bumpCounter("zzz-unique");
  return body.items.map((x) => x.id);
}
"#,
        distractor: r#"function fib(n) {
  if (n < 2) {
    return n;
  }
  return fib(n - 1) + fib(n - 2);
}
"#,
        query: "const res = await fetch(url);\nif (!res.ok) {\n  throw new Error(`HTTP ${res.status}`);\n}\n",
        keeps: &["const body = await res.json();", "return body.items.map((x) => x.id);"],
        drops: &["auditTrail", "bumpCounter"],
    },
    LangCase {
        lang: Lang::TypeScript,
        ext: "ts",
        a: r#"export function debounceA<T extends (...args: any[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return ((...args: any[]) => {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
    auditTrail(4242);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}
"#,
        b: r#"export function debounceB<T extends (...args: any[]) => void>(cb: T, wait: number): T {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return ((...args: any[]) => {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
    metrics.bumpCounter("zzz-unique");
    timer = setTimeout(() => cb(...args), wait);
  }) as T;
}
"#,
        distractor: r#"export function fib(n: number): number {
  if (n < 2) {
    return n;
  }
  return fib(n - 1) + fib(n - 2);
}
"#,
        query: "let timer: ReturnType<typeof setTimeout> | undefined;\n",
        keeps: &["clearTimeout(timer);", "timer = setTimeout(() => fn(...args), ms);"],
        drops: &["auditTrail", "bumpCounter"],
    },
    LangCase {
        lang: Lang::Tsx,
        ext: "tsx",
        a: r#"export function RowsA(props: Props) {
  const items = props.items.filter((x) => x.visible);
  const total = items.length;
  const label = formatCount(total);
  auditTrail(4242);
  return <div className="rows">{label}</div>;
}
"#,
        b: r#"export function RowsB(props: Props) {
  const items = props.items.filter((x) => x.visible);
  const total = items.length;
  const label = formatCount(total);
  metrics.bumpCounter("zzz-unique");
  return <div className="rows">{label}</div>;
}
"#,
        distractor: r#"export function Fib({ n }: { n: number }) {
  return <span>{n < 2 ? n : n * 2}</span>;
}
"#,
        query: "const items = props.items.filter((x) => x.visible);\n",
        keeps: &["const total = items.length;", "const label = formatCount(total);", "return <div className=\"rows\">{label}</div>;"],
        drops: &["auditTrail", "bumpCounter"],
    },
    LangCase {
        lang: Lang::Go,
        ext: "go",
        a: r#"package a

func ReadA(path string) ([]string, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var lines []string
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		lines = append(lines, scanner.Text())
		auditTrail(4242)
	}
	return lines, scanner.Err()
}
"#,
        b: r#"package b

func ReadB(name string) ([]string, error) {
	f, err := os.Open(name)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var lines []string
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		lines = append(lines, scanner.Text())
		metrics.BumpCounter("zzz-unique")
	}
	return lines, scanner.Err()
}
"#,
        distractor: r#"package c

func Fib(n int) int {
	if n < 2 {
		return n
	}
	return Fib(n-1) + Fib(n-2)
}
"#,
        query: "f, err := os.Open(path)\nif err != nil {\n\treturn nil, err\n}\n",
        keeps: &["defer f.Close()", "scanner := bufio.NewScanner(f)", "for scanner.Scan() {", "return lines, scanner.Err()"],
        drops: &["auditTrail", "BumpCounter"],
    },
    LangCase {
        lang: Lang::Rust,
        ext: "rs",
        a: r#"pub fn count_a(path: &Path) -> io::Result<HashMap<String, usize>> {
    let text = fs::read_to_string(path)?;
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word.to_lowercase()).or_insert(0) += 1;
        audit_trail(4242);
    }
    Ok(counts)
}
"#,
        b: r#"pub fn count_b(file: &Path) -> io::Result<HashMap<String, usize>> {
    let text = fs::read_to_string(file)?;
    let mut counts = HashMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word.to_lowercase()).or_insert(0) += 1;
        metrics::bump_counter("zzz-unique");
    }
    Ok(counts)
}
"#,
        distractor: r#"pub fn fib(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}
"#,
        query: "let text = fs::read_to_string(path)?;\nlet mut counts = HashMap::new();\n",
        keeps: &["for word in text.split_whitespace() {", "*counts.entry(word.to_lowercase()).or_insert(0) += 1;", "Ok(counts)"],
        drops: &["audit_trail", "bump_counter"],
    },
];

#[test]
fn all_languages_end_to_end() {
    let dir = scratch("langs");
    for case in CASES {
        let corpus = dir.path().join(format!("corpus-{}", case.lang));
        write(&corpus, &format!("ra/src/a.{}", case.ext), case.a);
        write(&corpus, &format!("rb/src/b.{}", case.ext), case.b);
        write(&corpus, &format!("rc/src/c.{}", case.ext), case.distractor);
        let engine = engine_for(&corpus, &[case.lang], &dir.path().join(format!("{}.sil", case.lang)));
        let lang = case.lang;

        // Search: both similar functions outrank the distractor.
        let ranked = engine.search(lang, case.query, 10).unwrap();
        assert!(ranked.len() >= 2, "{lang}: {} results", ranked.len());
        let top2: Vec<String> = ranked[..2].iter().map(|r| method_name(&engine, r.id)).collect();
        assert!(top2.iter().all(|n| !n.eq_ignore_ascii_case("fib")), "{lang}: {top2:?}");
        assert!(ranked[0].score > 0.9, "{lang}: top score {}", ranked[0].score);

        let out = engine.recommend(lang, case.query).unwrap();
        let rec = out
            .recommendations
            .iter()
            .find(|r| r.cluster.len() >= 2)
            .unwrap_or_else(|| panic!("{lang}: no multi-method cluster ({} recs)", out.recommendations.len()));
        let plain = render(&rec.tree, &rec.query_match, Some(case.query), Style::Plain);
        for k in case.keeps {
            assert!(plain.lines().any(|l| l.trim() == *k), "{lang}: missing {k:?} in\n{plain}");
        }
        for d in case.drops {
            assert!(!plain.contains(d), "{lang}: should drop {d:?}:\n{plain}");
        }
        // The dropped per-function line becomes this language's placeholder.
        let lay = layout(&rec.tree, &rec.query_match, Some(case.query));
        let ph: Vec<&str> = lay.lines.iter().filter(|l| l.placeholder).map(|l| l.text.trim()).collect();
        assert!(!ph.is_empty(), "{lang}: no placeholder in\n{plain}");
        assert!(ph.iter().all(|p| *p == lang.placeholder()), "{lang}: {ph:?}");
        assert!(lay.lines.iter().any(|l| l.extra && !l.placeholder), "{lang}: nothing highlighted");
        // Query lines are never marked extra.
        let first_q = case.query.lines().next().unwrap().trim();
        if let Some(l) = lay.lines.iter().find(|l| l.text.trim() == first_q) {
            assert!(!l.extra, "{lang}: query line marked extra");
        }
        for style in [Style::Ansi, Style::Html, Style::Json] {
            assert!(!render(&rec.tree, &rec.query_match, Some(case.query), style).is_empty());
        }
        let json: serde_json::Value =
            serde_json::from_str(&render(&rec.tree, &rec.query_match, None, Style::Json)).unwrap();
        assert_eq!(json["lang"], lang.as_str());
    }
    assert_eq!(Lang::Python.placeholder(), "...");
    assert_eq!(Lang::Go.placeholder(), "// ...");
}

/// The single-pass indexer equals walk + `InvertedIndex::build`, and the bounded
/// Phase II (`search`, `recommend`) equals exhaustive reranking, on every language.
#[test]
fn single_pass_index_and_bounded_rerank_are_exact() {
    let dir = scratch("exact");
    let mut corpora: Vec<(Vec<Lang>, std::path::PathBuf, Vec<&str>)> = Vec::new();
    let paper = dir.path().join("paper");
    write_paper_corpus(&paper);
    corpora.push((vec![Lang::Java], paper, vec![LISTING4, "view.getChildAt(i);", "for (int i = 0; i < n; i++) {"]));
    for case in CASES {
        let corpus = dir.path().join(format!("corpus-{}", case.lang));
        write(&corpus, &format!("ra/src/a.{}", case.ext), case.a);
        write(&corpus, &format!("rb/src/b.{}", case.ext), case.b);
        write(&corpus, &format!("rc/src/c.{}", case.ext), case.distractor);
        corpora.push((vec![case.lang], corpus, vec![case.query]));
    }
    for (langs, root, queries) in corpora {
        let (c1, i1, _) = sillage_index::build_corpus_and_index(&root, &langs, &AromaFeaturizer).unwrap();
        let (c2, _) = sillage_index::build_corpus(&root, &langs).unwrap();
        let i2 = sillage_index::InvertedIndex::build(&c2, &AromaFeaturizer);
        assert_eq!(c1.methods, c2.methods, "{langs:?}");
        assert_eq!(i1, i2, "{langs:?}");
        let mut params = sillage_core::Params::default();
        params.eta2 = 2; // force the bound to prune candidates
        let engine = sillage_engine::Engine::new(c1, i1, sillage_index::Bm25::build(&c2), Box::new(AromaFeaturizer), params);
        for q in queries {
            for n in [1, 2, 3, 100] {
                let a = engine.search(langs[0], q, n).unwrap();
                let b = engine.search_exhaustive(langs[0], q, n).unwrap();
                let key = |r: &sillage_engine::Ranked| (r.id, r.sim, r.overlap, r.pruned.keep.clone());
                assert_eq!(a.iter().map(key).collect::<Vec<_>>(), b.iter().map(key).collect::<Vec<_>>(), "{langs:?} n={n} {q:?}");
            }
        }
    }
}

/// Comments are not tree nodes: rendering a full body keeps them verbatim (no
/// `// ...` placeholder), while a dropped code line still becomes one, even when
/// it starts like a comment continuation (`*deref`).
#[test]
fn comments_render_verbatim_and_drops_use_placeholders() {
    let src = "fn f(m: &mut HashMap<u32, u32>, k: u32) {\n    // count it\n    let v = 1;\n    *m.entry(k).or_insert(0) += v;\n    /* block\n     * comment */\n    log(v);\n}\n";
    let ms = sillage_parse::extract_methods(Lang::Rust, src);
    let t = &ms[0].tree;
    let all = vec![true; t.leaves.len()];
    let out = render(t, &all, None, Style::Plain);
    // (A comment before the first terminal lies outside the rendered range.)
    assert_eq!(out, "let v = 1;\n*m.entry(k).or_insert(0) += v;\n/* block\n * comment */\nlog(v);\n");
    assert!(layout(t, &all, None).lines.iter().all(|l| !l.placeholder));
    // Drop the `*m.entry(...)` statement.
    let keep: Vec<bool> = t.leaves.iter().map(|&l| !matches!(t.label(l), "m" | "entry" | "k" | "or_insert" | "0") && !(t.label(l) == "v" && t.node(l).span.start > 60)).collect();
    let pruned = t.retain(&keep);
    let out = render(&pruned, &vec![true; pruned.leaves.len()], None, Style::Plain);
    assert!(out.contains("// ...") && !out.contains("entry"), "{out}");
}

/// Stage 5 regression: one index holding Java, Python and JavaScript only ever
/// answers a query with methods (and clusters) of the query's language, even
/// when other languages share many of its tokens and η1 is tiny.
#[test]
fn mixed_language_index_stays_in_language() {
    let dir = scratch("mixed");
    let corpus = dir.path().join("corpus");
    for case in CASES.iter().filter(|c| matches!(c.lang, Lang::Java | Lang::Python | Lang::JavaScript)) {
        write(&corpus, &format!("{}-a/src/a.{}", case.lang, case.ext), case.a);
        write(&corpus, &format!("{}-b/src/b.{}", case.lang, case.ext), case.b);
        write(&corpus, &format!("{}-c/src/c.{}", case.lang, case.ext), case.distractor);
    }
    // Look-alikes in other languages sharing the Python query's identifiers.
    write(
        &corpus,
        "lookalike-java/src/L.java",
        "class L {\n    Object open(String path) {\n        Object fh = open(path);\n        Object data = json.load(fh);\n        return data;\n    }\n}\n",
    );
    write(
        &corpus,
        "lookalike-js/src/l.js",
        "function openIt(path) {\n  const fh = open(path);\n  const data = json.load(fh);\n  return data;\n}\n",
    );
    let langs = [Lang::Java, Lang::Python, Lang::JavaScript];
    let mut engine = engine_for(&corpus, &langs, &dir.path().join("mixed.sil"));
    let lang_of = |e: &sillage_engine::Engine, id: sillage_core::MethodId| e.corpus.methods[id.0 as usize].lang;
    let all_langs: std::collections::BTreeSet<Lang> = engine.corpus.methods.iter().map(|m| m.lang).collect();
    assert_eq!(all_langs.len(), 3);

    for eta1 in [2, 3, 1000] {
        engine.params.eta1 = eta1;
        for case in CASES.iter().filter(|c| matches!(c.lang, Lang::Java | Lang::Python | Lang::JavaScript)) {
            let lang = case.lang;
            let ranked = engine.search(lang, case.query, 100).unwrap();
            assert!(!ranked.is_empty(), "{lang} eta1={eta1}");
            assert!(ranked.iter().all(|r| lang_of(&engine, r.id) == lang), "{lang} eta1={eta1}: cross-language result");
            let ex = engine.search_exhaustive(lang, case.query, 100).unwrap();
            assert!(ex.iter().all(|r| lang_of(&engine, r.id) == lang));
            let feats = engine.features_only(lang, case.query, 100).unwrap();
            assert!(feats.iter().all(|&id| lang_of(&engine, id) == lang));
            let bm25 = engine.bm25_search(lang, case.query, 100);
            assert!(bm25.iter().all(|&id| lang_of(&engine, id) == lang));
            let out = engine.recommend(lang, case.query).unwrap();
            assert!(out.n2.iter().all(|r| lang_of(&engine, r.id) == lang));
            for rec in &out.recommendations {
                assert!(rec.cluster.iter().all(|&id| lang_of(&engine, id) == lang), "{lang}: mixed cluster");
            }
            if eta1 == 1000 {
                // η1 slots are not wasted: the two same-language look-alikes reach N2.
                assert!(out.n2.len() >= 2, "{lang}: n2 = {}", out.n2.len());
                assert!(out.recommendations.iter().any(|r| r.cluster.len() >= 2), "{lang}: no multi-method cluster");
            }
        }
    }
    // With η1 = 2, the Python query's top Phase I slots are both Python methods.
    engine.params.eta1 = 2;
    let py = CASES.iter().find(|c| c.lang == Lang::Python).unwrap();
    let out = engine.recommend(Lang::Python, py.query).unwrap();
    assert_eq!(out.n2.len(), 2, "both η1 slots go to Python methods");
}

/// Stage 5 regression: `recommend_excluding` drops the excluded methods before
/// Phase II, so they occupy no N2, cluster or top-k slot.
#[test]
fn recommend_excluding_drops_methods_before_phase_two() {
    let dir = scratch("exclude");
    let corpus = dir.path().join("corpus");
    write_paper_corpus(&corpus);
    let mut engine = engine_for(&corpus, &[Lang::Java], &dir.path().join("x.sil"));
    let full = engine.recommend(Lang::Java, LISTING4).unwrap();
    let top = full.n2[0].id;
    let out = engine.recommend_excluding(Lang::Java, LISTING4, &[top]).unwrap();
    assert!(out.n2.iter().all(|r| r.id != top));
    assert!(out.recommendations.iter().all(|r| !r.cluster.contains(&top)));
    // The freed slot is reused: with η1 = |N2 of the full run|, the excluded
    // method does not cost the excluded run a candidate.
    engine.params.eta1 = full.n2.len().max(1);
    let narrow = engine.recommend_excluding(Lang::Java, LISTING4, &[top]).unwrap();
    assert!(narrow.n2.iter().all(|r| r.id != top));
    assert_eq!(engine.recommend_excluding(Lang::Java, LISTING4, &[]).unwrap().n2.len(), engine.recommend(Lang::Java, LISTING4).unwrap().n2.len());
}
