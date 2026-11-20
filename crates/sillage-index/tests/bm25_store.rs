//! BM25 keyword baseline and save/load roundtrip.

mod common;

use std::sync::Arc;

use sillage_core::{FeatureBag, Featurizer, Lang, MethodId, MethodRecord, SimplifiedTree, Span};
use sillage_index::{AromaFeaturizer, Bm25, Corpus, InvertedIndex, load, save};

fn method(id: u32, body: &str) -> MethodRecord {
    let text = format!("void m{id}() {{{body}}}");
    let start = format!("void m{id}() {{").len() as u32;
    MethodRecord {
        id: MethodId(id),
        lang: Lang::Java,
        repo: "repo".into(),
        path: format!("src/F{id}.java").into(),
        name: format!("m{id}").into(),
        line_start: id + 1,
        body: Span::new(start, text.len() as u32 - 1),
        text: Arc::from(text),
    }
}

fn corpus() -> Corpus {
    Corpus {
        methods: vec![
            method(0, " int total = 0; for (int i = 0; i < n; i++) total += i; "),
            method(1, " view.getChildCount(); ViewGroup group = (ViewGroup) view; "),
            method(2, " if (view instanceof ViewGroup) { hideKeyboard(view); } "),
            method(3, " return a + b; "),
        ],
    }
}

#[test]
fn bm25_ranks_relevant_methods() {
    let bm25 = Bm25::build(&corpus());
    let hits = bm25.search("view instanceof ViewGroup", 10);
    let ids: Vec<u32> = hits.iter().map(|h| h.0.0).collect();
    assert_eq!(ids, [2, 1], "{hits:?}");
    assert!(hits[0].1 > hits[1].1 && hits[1].1 > 0.0);

    // camelCase sub-words match.
    let hits = bm25.search("child count", 10);
    assert_eq!(hits.first().map(|h| h.0), Some(MethodId(1)));

    // Top-n truncation keeps the best.
    assert_eq!(bm25.search("view instanceof ViewGroup", 1).len(), 1);
    assert_eq!(bm25.search("view instanceof ViewGroup", 1)[0].0, MethodId(2));
}

#[test]
fn bm25_indexes_body_not_signature() {
    let bm25 = Bm25::build(&corpus());
    // `m3` / `void` occur only in signatures.
    assert!(bm25.search("m3", 10).is_empty());
    assert!(bm25.search("void", 10).is_empty());
}

#[test]
fn bm25_empty_cases() {
    let bm25 = Bm25::build(&corpus());
    assert!(bm25.search("", 10).is_empty());
    assert!(bm25.search("  ;;( ) ", 10).is_empty());
    assert!(bm25.search("zzzunknown", 10).is_empty());
    assert!(bm25.search("view", 0).is_empty());

    let empty = Bm25::build(&Corpus::default());
    assert!(empty.search("view", 10).is_empty());
    assert_eq!(empty.num_terms(), 0);
}

#[test]
fn bm25_ties_break_by_method_id() {
    let docs = [(MethodId(7), "alpha beta"), (MethodId(3), "alpha beta"), (MethodId(5), "gamma")];
    let bm25 = Bm25::build_from_texts(&docs);
    let hits = bm25.search("alpha", 10);
    assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), [MethodId(3), MethodId(7)]);
    assert_eq!(hits[0].1, hits[1].1);
}

#[test]
fn bm25_prefers_rarer_terms_and_shorter_docs() {
    let docs = [
        (MethodId(0), "common common rare"),
        (MethodId(1), "common"),
        (MethodId(2), "common filler filler filler filler filler rare"),
    ];
    let bm25 = Bm25::build_from_texts(&docs);
    let hits = bm25.search("rare", 10);
    assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), [MethodId(0), MethodId(2)]);
    let rare = bm25.search("rare", 1)[0].1;
    let common = bm25.search("common", 1)[0].1;
    assert!(rare > common);
}

#[test]
fn save_load_roundtrip() {
    let corpus = corpus();
    let tree: SimplifiedTree = common::listing4();
    let index = InvertedIndex::build_from_trees(&[(MethodId(2), &tree)], corpus.methods.len(), &AromaFeaturizer);
    let bm25 = Bm25::build(&corpus);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corpus.sillage");
    save(&path, &corpus, &index, &bm25).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"SILLAGE1");
    assert_eq!(&bytes[8..12], &sillage_index::FORMAT_VERSION.to_le_bytes());

    let (c2, i2, b2) = load(&path).unwrap();
    assert_eq!(c2.methods, corpus.methods);
    assert_eq!(i2, index);
    assert_eq!(b2, bm25);

    let q: FeatureBag = AromaFeaturizer.featurize(&tree).total();
    assert_eq!(i2.search(&q, 5), index.search(&q, 5));
    assert_eq!(b2.search("view", 5), bm25.search("view", 5));
    assert!(!dir.path().join("corpus.sillage.tmp").exists());
}

#[test]
fn load_rejects_bad_files() {
    let dir = tempfile::tempdir().unwrap();

    let bad_magic = dir.path().join("a");
    std::fs::write(&bad_magic, b"NOTSILLAGE......").unwrap();
    assert!(load(&bad_magic).unwrap_err().to_string().contains("magic"));

    let short = dir.path().join("b");
    std::fs::write(&short, b"SILL").unwrap();
    assert!(load(&short).is_err());

    let bad_version = dir.path().join("c");
    let mut bytes = b"SILLAGE1".to_vec();
    bytes.extend_from_slice(&99u32.to_le_bytes());
    std::fs::write(&bad_version, &bytes).unwrap();
    assert!(load(&bad_version).unwrap_err().to_string().contains("version 99"));

    let truncated = dir.path().join("d");
    let mut bytes = b"SILLAGE1".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.push(200);
    std::fs::write(&truncated, &bytes).unwrap();
    assert!(load(&truncated).is_err());

    assert!(load(&dir.path().join("missing")).is_err());
}
