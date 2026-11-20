//! Smoke tests of the real `sillage` binary (`std::process::Command`).

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::*;

fn sillage(args: &[&str], stdin: Option<&str>) -> Output {
    use std::io::Write;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sillage"));
    cmd.args(args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn sillage");
    if let Some(s) = stdin {
        child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}

fn ok(args: &[&str], stdin: Option<&str>) -> String {
    let out = sillage(args, stdin);
    assert!(
        out.status.success(),
        "sillage {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Paper corpus plus 12 generated 14-line methods, so the recall (≥12 lines)
/// and quality (≥8 lines) samplers have material.
fn write_bench_corpus(root: &Path) {
    write_paper_corpus(root);
    let mut src = String::from("package gen;\n\nclass Gen {\n");
    for k in 0..12 {
        src.push_str(&format!("    int work{k}(List<Integer> xs, Map<String, Integer> m) {{\n"));
        src.push_str("        int total = 0;\n");
        for s in 0..12 {
            match (k + s) % 4 {
                0 => src.push_str(&format!("        total += xs.get({s}) * {k};\n")),
                1 => src.push_str(&format!("        m.put(\"key{s}\", total + {k});\n")),
                2 => src.push_str(&format!("        if (total > {s}) {{ total -= m.getOrDefault(\"k{k}\", {s}); }}\n")),
                _ => src.push_str(&format!("        for (int i = 0; i < {s}; i++) {{ total ^= xs.size() + i; }}\n")),
            }
        }
        src.push_str("        return total;\n    }\n\n");
    }
    src.push_str("}\n");
    write(root, "gen/src/Gen.java", &src);
}

#[test]
fn cli_end_to_end() {
    let dir = scratch("cli");
    let corpus = dir.path().join("corpus");
    write_bench_corpus(&corpus);
    let index = dir.path().join("java.sil");
    let (c, i) = (corpus.to_str().unwrap(), index.to_str().unwrap());
    let q = dir.path().join("listing4.java");
    std::fs::write(&q, LISTING4).unwrap();
    let qs = q.to_str().unwrap();

    let out = ok(&["index", "--corpus", c, "--out", i, "--lang", "java"], None);
    assert!(out.starts_with("indexed 20 methods"), "{out}");
    assert!(index.exists());

    let out = ok(&["stats", "--index", i], None);
    assert!(out.contains("methods: 20") && out.contains("java        20") && out.contains("repos: 4"), "{out}");

    let out = ok(&["explain", qs], None);
    assert!(out.contains("\"if##\"") && out.contains("(0, 1, int#)") && out.contains("(#VAR, getChildCount)"), "{out}");
    // stdin + --lang
    let out = ok(&["explain", "--lang", "python", "-"], Some("x = foo(y)\n"));
    assert!(out.contains("python simplified parse tree"), "{out}");

    let out = ok(&["search", "--index", i, qs, "--n", "3"], None);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert!(lines[..2].iter().all(|l| l.contains("setupUI")), "{out}");
    let out = ok(&["search", "--index", i, qs, "--n", "1", "--show", "--format", "plain"], None);
    assert!(out.contains("View innerView = ((ViewGroup) view).getChildAt(i);"), "{out}");

    // query in every format
    let plain = ok(&["query", "--index", i, qs, "--format", "plain"], None);
    assert!(plain.contains("== Recommendation 1 (cluster of 2"), "{plain}");
    let first = plain.split("== Recommendation 2").next().unwrap();
    assert!(first.contains("setupUIToHideKeyBoardOnTouch(innerView);") && !first.contains("hideKeyBoard();"), "{plain}");
    let ansi = ok(&["query", "--index", i, qs, "--format", "ansi", "--k", "1"], None);
    assert!(ansi.contains("\x1b[1;32m+ if (!(view instanceof EditText)) {"), "{ansi}");
    assert_eq!(ansi.matches("== Recommendation").count(), 1);
    let html = ok(&["query", "--index", i, qs, "--format", "html"], None);
    assert!(html.contains("<pre class=\"sillage-rec\""), "{html}");
    let json = ok(&["query", "--index", i, qs, "--format", "json"], None);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let recs = v["recommendations"].as_array().unwrap();
    assert!(!recs.is_empty());
    assert_eq!(recs[0]["cluster"].as_array().unwrap().len(), 2);
    assert!(recs[0]["render"]["code"].as_str().unwrap().contains("return false;"));
    assert!(v["timings"]["total_ms"].as_f64().unwrap() >= 0.0);
    // stdin query with tuning flags
    let out = ok(&["query", "--index", i, "--lang", "java", "--format", "plain", "--tau1", "0.99", "-"], Some(LISTING4));
    assert!(out.contains("== Recommendation 1"), "{out}");

    let out = ok(&["bench", "recall", "--index", i, "--n", "10", "--bootstrap", "50", "--seed", "7"], None);
    assert!(out.contains("| Sillage |") && out.contains("BM25 Keywords") && out.contains("Features Search"), "{out}");
    let out = ok(&["bench", "recall", "--index", i, "--n", "10", "--mode", "noncontiguous", "--bootstrap", "50", "--json"], None);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 3);
    assert!(v["sampled"].as_u64().unwrap() >= 10);
    let r1 = v["rows"][2]["recall_at_100"]["mean"].as_f64().unwrap();
    assert!(r1 > 0.9, "Sillage Recall@100 on the toy corpus: {r1}");

    let out = ok(&["bench", "quality", "--index", i, "--n", "10", "--seed", "7"], None);
    assert!(out.contains("Containment rate") && out.contains("| phase1 |"), "{out}");
    let out = ok(&["bench", "quality", "--index", i, "--n", "10", "--json"], None);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["sampled"].as_u64().unwrap() > 0);
    assert_eq!(v["phases"].as_array().unwrap().len(), 4);

    // Errors are reported, not panics.
    let bad = sillage(&["stats", "--index", dir.path().join("missing.sil").to_str().unwrap()], None);
    assert!(!bad.status.success());
    let bad = sillage(&["explain", "-"], Some("x"));
    assert!(!bad.status.success() && String::from_utf8_lossy(&bad.stderr).contains("--lang"));
}
