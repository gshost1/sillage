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
    // Stage 3: cluster members carry metadata, lines carry file line numbers.
    let m0 = &recs[0]["cluster"][0];
    assert!(m0["repo"].is_string() && m0["path"].is_string() && m0["name"].is_string(), "{m0}");
    let start = m0["line_start"].as_u64().unwrap();
    let line0 = &recs[0]["render"]["lines"][0];
    assert_eq!(line0["file_line"].as_u64().unwrap(), line0["src_line"].as_u64().unwrap() + start - 1, "{line0}");
    // --max-lines truncates with a trailing placeholder.
    let json = ok(&["query", "--index", i, qs, "--format", "json", "--max-lines", "2"], None);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let lines = v["recommendations"][0]["render"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[2]["text"].as_str().unwrap().contains("more line"), "{lines:?}");
    let plain = ok(&["query", "--index", i, qs, "--format", "plain", "--max-lines", "2", "--k", "1"], None);
    assert!(plain.contains("// ... ("), "{plain}");
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

/// Stage 5 regressions: `search --format json` is one valid JSON document,
/// indexing a missing / empty corpus fails, invalid parameters are rejected,
/// and a missing `--lang` fails without waiting for stdin.
#[test]
fn cli_stage5_regressions() {
    let dir = scratch("cli5");
    let corpus = dir.path().join("corpus");
    write_paper_corpus(&corpus);
    let index = dir.path().join("java.sil");
    let (c, i) = (corpus.to_str().unwrap(), index.to_str().unwrap());
    ok(&["index", "--corpus", c, "--out", i, "--lang", "java"], None);
    let q = dir.path().join("listing4.java");
    std::fs::write(&q, LISTING4).unwrap();
    let qs = q.to_str().unwrap();

    // search --format json, with and without --show.
    let out = ok(&["search", "--index", i, qs, "--n", "3", "--format", "json"], None);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(v["lang"], "java");
    let rows = v["results"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for (k, r) in rows.iter().enumerate() {
        assert_eq!(r["rank"], k + 1);
        for key in ["id", "repo", "path", "name", "line_start", "score", "sim", "overlap"] {
            assert!(!r[key].is_null(), "{key} missing in {r}");
        }
        assert!(r.get("render").is_none());
    }
    let out = ok(&["search", "--index", i, qs, "--n", "2", "--format", "json", "--show", "--max-lines", "3"], None);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
    let r0 = &v["results"][0];
    let lines = r0["render"]["lines"].as_array().unwrap();
    assert!(lines.len() <= 4, "{lines:?}");
    let start = r0["line_start"].as_u64().unwrap();
    assert_eq!(lines[0]["file_line"].as_u64().unwrap(), lines[0]["src_line"].as_u64().unwrap() + start - 1);
    // stdin search with --lang, plain --show with --max-lines.
    let out = ok(&["search", "--index", i, "--lang", "java", "--n", "1", "--show", "--format", "plain", "--max-lines", "2", "-"], Some(LISTING4));
    assert!(out.contains("more line"), "{out}");

    // Missing / empty corpus.
    let missing = dir.path().join("nonexistent");
    let bad = sillage(&["index", "--corpus", missing.to_str().unwrap(), "--out", dir.path().join("m.sil").to_str().unwrap()], None);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("not accessible"), "{}", String::from_utf8_lossy(&bad.stderr));
    assert!(!dir.path().join("m.sil").exists());
    let bad = sillage(&["index", "--corpus", "/nonexistent", "--out", dir.path().join("n.sil").to_str().unwrap()], None);
    assert!(!bad.status.success());
    let empty = dir.path().join("empty");
    write(&empty, "docs/README.md", "# nothing to index\n");
    let bad = sillage(&["index", "--corpus", empty.to_str().unwrap(), "--out", dir.path().join("e.sil").to_str().unwrap()], None);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("no methods found"), "{}", String::from_utf8_lossy(&bad.stderr));

    // Parameter validation.
    for (flag, val) in [
        ("--tau1", "NaN"),
        ("--tau1", "1.5"),
        ("--tau2", "inf"),
        ("--tau2", "-1"),
        ("--tau3", "-0.1"),
        ("--k", "0"),
        ("--eta1", "0"),
        ("--eta2", "0"),
        ("--max-lines", "0"),
    ] {
        let bad = sillage(&["query", "--index", i, qs, flag, val], None);
        assert!(!bad.status.success(), "{flag} {val} accepted");
    }

    // `.pyi` resolves like `sillage index` does.
    let pyi = dir.path().join("stub.pyi");
    std::fs::write(&pyi, "x = foo(y)\n").unwrap();
    let out = ok(&["explain", pyi.to_str().unwrap()], None);
    assert!(out.contains("python simplified parse tree"), "{out}");

    // Missing --lang on stdin fails at once, with stdin still open.
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_sillage"))
        .args(["query", "--index", i])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _stdin = child.stdin.take(); // kept open: no EOF
    let t = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if t.elapsed().as_secs() > 20 {
            child.kill().unwrap();
            panic!("sillage waited for stdin before reporting the missing --lang");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(!status.success());
}
