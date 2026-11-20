//! Stage 1 (agent engine): incremental directory refresh, the import table and
//! test flags produced by corpus construction.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use sillage_core::{Featurizer, Lang, MethodId};
use sillage_index::{AromaFeaturizer, IncrementalIndex, IndexParts, index_dir, index_files};

const A: &str = "use std::collections::HashMap;\nuse std::io::{self, Read};\n\npub fn count(words: &[String]) -> HashMap<String, usize> {\n    let mut m = HashMap::new();\n    for w in words {\n        *m.entry(w.clone()).or_insert(0) += 1;\n    }\n    m\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn counts() {\n        let m = count(&[\"a\".to_string()]);\n        assert_eq!(m[\"a\"], 1);\n    }\n}\n";
const B: &str = "pub fn read_all(r: &mut dyn std::io::Read) -> std::io::Result<String> {\n    let mut s = String::new();\n    r.read_to_string(&mut s)?;\n    Ok(s)\n}\n";
const B2: &str = "pub fn read_all(r: &mut dyn std::io::Read) -> std::io::Result<String> {\n    let mut s = String::new();\n    let n = r.read_to_string(&mut s)?;\n    if n == 0 {\n        return Ok(String::new());\n    }\n    Ok(s)\n}\n";
const C: &str = "pub fn total(xs: &[u32]) -> u32 {\n    let mut t = 0;\n    for x in xs {\n        t += x;\n    }\n    t\n}\n";
const T: &str = "use mycrate::count;\n\n#[test]\nfn integration() {\n    let m = count(&[]);\n    assert!(m.is_empty());\n}\n";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn age(root: &Path, rel: &str) {
    let f = fs::File::options().write(true).open(root.join(rel)).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
}

fn scratch(root: &Path) -> IndexParts {
    index_dir(root, Lang::ALL, &AromaFeaturizer).unwrap().0
}

fn assert_same(a: &IndexParts, b: &IndexParts) {
    assert_eq!(a.corpus, b.corpus);
    assert_eq!(a.index, b.index);
    assert_eq!(a.bm25, b.bm25);
}

fn names(p: &IndexParts) -> Vec<String> {
    p.corpus.methods.iter().map(|m| format!("{}:{}", m.rel_path(), m.name)).collect()
}

#[test]
fn corpus_carries_import_table_and_test_flags() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/a.rs", A);
    write(root, "src/b.rs", B);
    write(root, "tests/t.rs", T);
    let parts = scratch(root);
    let c = &parts.corpus;
    let by_name = |n: &str| c.methods.iter().find(|m| &*m.name == n).unwrap();

    assert_eq!(names(&parts), ["src/a.rs:count", "src/a.rs:counts", "src/b.rs:read_all", "tests/t.rs:integration"]);
    assert!(!by_name("count").is_test);
    assert!(by_name("counts").is_test, "#[test] inside #[cfg(test)] mod");
    assert!(!by_name("read_all").is_test);
    assert!(by_name("integration").is_test, "tests/ directory");

    // One import-table entry per file; methods of one file share it.
    assert_eq!(c.file_imports.len(), 3);
    assert_eq!(by_name("count").file_id, by_name("counts").file_id);
    assert_eq!(
        c.imports_of(by_name("count")),
        ["use std::collections::HashMap;".into(), "use std::io::{self, Read};".into(), "use super::*;".into()] as [Box<str>; 3]
    );
    assert!(c.imports_of(by_name("read_all")).is_empty());
    assert_eq!(c.imports_of(by_name("integration")), ["use mycrate::count;".into()] as [Box<str>; 1]);
    c.validate().unwrap();

    // index_files over the same files gives the same index.
    let files = ["src/a.rs", "src/b.rs", "tests/t.rs"].map(|p| root.join(p)).to_vec();
    let (listed, stats) = index_files(root, &files, Lang::ALL, &AromaFeaturizer).unwrap();
    assert_eq!(stats.files, 3);
    assert_same(&listed, &parts);
    let (subset, _) = index_files(root, &[root.join("src/b.rs"), "tests/t.rs".into()], Lang::ALL, &AromaFeaturizer).unwrap();
    assert_eq!(names(&subset), ["src/b.rs:read_all", "tests/t.rs:integration"]);
    // Paths escaping the root are never read.
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "evil.rs", C);
    let escape = Path::new("..").join(outside.path().file_name().unwrap()).join("evil.rs");
    let (none, _) = index_files(root, &[escape, outside.path().join("evil.rs")], Lang::ALL, &AromaFeaturizer).unwrap();
    assert!(none.corpus.methods.is_empty());
}

#[test]
fn incremental_refresh_tracks_adds_edits_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/a.rs", A);
    write(root, "src/b.rs", B);
    write(root, "tests/t.rs", T);
    for f in ["src/a.rs", "src/b.rs", "tests/t.rs"] {
        age(root, f);
    }

    let mut inc = IncrementalIndex::new(root, Lang::ALL, Box::new(AromaFeaturizer)).unwrap();
    let r = inc.refresh().unwrap();
    assert_eq!((r.stats.files_added, r.stats.files_modified, r.stats.files_removed, r.stats.files_unchanged), (3, 0, 0, 0));
    assert_eq!((r.stats.total_files, r.stats.total_methods), (3, 4));
    let first = r.parts.expect("first refresh always builds");
    assert_same(&first, &scratch(root));

    // Nothing changed: metadata is trusted (old mtimes), no file is re-read.
    let r = inc.refresh().unwrap();
    assert!(!r.stats.changed());
    assert!(r.parts.is_none());
    assert_eq!((r.stats.files_unchanged, r.stats.files_rehashed, r.stats.total_methods), (3, 0, 4));

    // Touching a file without changing its content re-hashes it but changes nothing.
    write(root, "src/b.rs", B);
    let r = inc.refresh().unwrap();
    assert!(!r.stats.changed() && r.parts.is_none());
    assert_eq!((r.stats.files_unchanged, r.stats.files_rehashed), (3, 1));

    // Modify b, add c, delete a.
    write(root, "src/b.rs", B2);
    write(root, "src/c.rs", C);
    fs::remove_file(root.join("src/a.rs")).unwrap();
    let r = inc.refresh().unwrap();
    assert_eq!((r.stats.files_added, r.stats.files_modified, r.stats.files_removed, r.stats.files_unchanged), (1, 1, 1, 1));
    assert_eq!((r.stats.total_files, r.stats.total_methods), (3, 3));
    assert!(r.stats.elapsed_ms >= 0.0);
    let parts = r.parts.expect("changed");
    let fresh = scratch(root);
    assert_same(&parts, &fresh);
    assert_eq!(names(&parts), ["src/b.rs:read_all", "src/c.rs:total", "tests/t.rs:integration"]);
    assert!(parts.corpus.methods[1].text.contains("t += x"));
    assert!(parts.corpus.methods[0].text.contains("if n == 0"));

    // Search results equal a from-scratch build.
    let q = sillage_parse::parse_snippet(Lang::Rust, "let mut t = 0;\nfor x in xs {\n    t += x;\n}").unwrap();
    let fq = AromaFeaturizer.featurize_query(&q).total();
    let hits = parts.index.search(&fq, 10);
    assert_eq!(hits, fresh.index.search(&fq, 10));
    assert_eq!(hits[0].0, MethodId(1));
    assert_eq!(parts.bm25.search("read_to_string", 5), fresh.bm25.search("read_to_string", 5));

    // A refresh after deleting everything yields an empty index.
    for f in ["src/b.rs", "src/c.rs", "tests/t.rs"] {
        fs::remove_file(root.join(f)).unwrap();
    }
    let r = inc.refresh().unwrap();
    assert_eq!((r.stats.files_removed, r.stats.total_files, r.stats.total_methods), (3, 0, 0));
    let empty = r.parts.unwrap();
    assert!(empty.corpus.methods.is_empty() && empty.corpus.file_imports.is_empty());
    assert_eq!(inc.num_files(), 0);
}

#[test]
fn incremental_refresh_keeps_dedup_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Identical files: only the first survives file-level dedup, like a full build.
    write(root, "src/c.rs", C);
    write(root, "src/copy_of_c.rs", C);
    let mut inc = IncrementalIndex::new(root, &[Lang::Rust], Box::new(AromaFeaturizer)).unwrap();
    let p = inc.refresh().unwrap().parts.unwrap();
    assert_same(&p, &scratch(root));
    assert_eq!(names(&p), ["src/c.rs:total"]);
    // Removing the survivor promotes the copy.
    fs::remove_file(root.join("src/c.rs")).unwrap();
    let p = inc.refresh().unwrap().parts.unwrap();
    assert_same(&p, &scratch(root));
    assert_eq!(names(&p), ["src/copy_of_c.rs:total"]);
    assert!(IncrementalIndex::new(root.join("missing"), &[Lang::Rust], Box::new(AromaFeaturizer)).is_err());
}
