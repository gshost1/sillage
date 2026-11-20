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

    // Whole, case-sensitive identifiers (Stage 3): `getChildCount` matches only
    // itself, not its sub-words or another casing.
    let hits = bm25.search("getChildCount", 10);
    assert_eq!(hits.first().map(|h| h.0), Some(MethodId(1)));
    assert!(bm25.search("child count", 10).is_empty());
    assert!(bm25.search("getchildcount", 10).is_empty());

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

    // Stage 5: pre-v4 files are rejected with a rebuild hint.
    let old = dir.path().join("old");
    let mut bytes = b"SILLAGE1".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 32]);
    std::fs::write(&old, &bytes).unwrap();
    let err = load(&old).unwrap_err().to_string();
    assert!(err.contains("version 3") && err.contains("sillage index"), "{err}");

    let truncated = dir.path().join("d");
    let mut bytes = b"SILLAGE1".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.push(200);
    std::fs::write(&truncated, &bytes).unwrap();
    assert!(load(&truncated).is_err());

    assert!(load(&dir.path().join("missing")).is_err());
}

fn valid_bytes() -> Vec<u8> {
    let corpus = corpus();
    let index = InvertedIndex::build(&corpus, &AromaFeaturizer);
    let bm25 = Bm25::build(&corpus);
    sillage_index::to_bytes(&corpus, &index, &bm25).unwrap()
}

/// Header: magic (8) + version (4) + xxh3 checksum (8).
const HEADER: usize = 20;

fn with_checksum(mut bytes: Vec<u8>) -> Vec<u8> {
    let sum = xxhash_rust::xxh3::xxh3_64(&bytes[HEADER..]);
    bytes[12..HEADER].copy_from_slice(&sum.to_le_bytes());
    bytes
}

/// Stage 5 regression: flipping any payload byte of a `.sil` file makes
/// `load` return an error (checksum mismatch), never panic; so does truncation.
#[test]
fn load_rejects_flipped_bytes_without_panicking() {
    let bytes = valid_bytes();
    assert!(sillage_index::from_bytes(&bytes, "ok").is_ok());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flipped.sil");
    for pos in (HEADER..bytes.len()).step_by(7).chain([bytes.len() - 1]) {
        let mut b = bytes.clone();
        b[pos] ^= 0x5a;
        std::fs::write(&path, &b).unwrap();
        let err = load(&path).unwrap_err().to_string();
        assert!(err.contains("checksum"), "byte {pos}: {err}");
    }
    // Corrupted checksum field itself.
    let mut b = bytes.clone();
    b[13] ^= 1;
    assert!(sillage_index::from_bytes(&b, "x").unwrap_err().to_string().contains("checksum"));
    // Truncated anywhere.
    for len in [0, 8, 12, 19, HEADER, bytes.len() / 2, bytes.len() - 1] {
        assert!(sillage_index::from_bytes(&bytes[..len], "x").is_err(), "len {len}");
    }
}

/// Stage 5 regression: even a corrupted payload with a *matching* checksum
/// (e.g. a hand-edited file) decodes to an error or a structure that passes
/// validation, and never panics.
#[test]
fn corrupted_payload_with_valid_checksum_never_panics() {
    let bytes = valid_bytes();
    let mut errors = 0;
    for pos in HEADER..bytes.len() {
        for mask in [0x01u8, 0x80, 0xff] {
            let mut b = bytes.clone();
            b[pos] ^= mask;
            let b = with_checksum(b);
            match std::panic::catch_unwind(|| sillage_index::from_bytes(&b, "x")) {
                Ok(Ok((c, i, m))) => {
                    // Whatever decoded must be safe to query.
                    let q = AromaFeaturizer.featurize(&common::listing4()).total();
                    let _ = i.search(&q, 10);
                    let _ = m.search("view ViewGroup", 10);
                    assert_eq!(i.num_methods(), c.methods.len());
                }
                Ok(Err(_)) => errors += 1,
                Err(_) => panic!("from_bytes panicked at byte {pos} mask {mask:#x}"),
            }
        }
    }
    assert!(errors > 0);
}

/// Stage 5: structural validation catches inconsistent parts that decode fine.
#[test]
fn load_validates_structures() {
    let good = corpus();
    let index = InvertedIndex::build(&good, &AromaFeaturizer);
    let bm25 = Bm25::build(&good);
    let check = |c: &Corpus, i: &InvertedIndex, b: &Bm25| {
        let bytes = sillage_index::to_bytes(c, i, b).unwrap();
        sillage_index::from_bytes(&bytes, "x").map(|_| ()).map_err(|e| format!("{e:#}"))
    };
    assert!(check(&good, &index, &bm25).is_ok());

    // Method ids out of order.
    let mut c = good.clone();
    c.methods.swap(0, 1);
    assert!(check(&c, &index, &bm25).unwrap_err().contains("id"));

    // Body span past the end of the text / off a char boundary.
    let mut c = good.clone();
    c.methods[2].body = Span::new(0, c.methods[2].text.len() as u32 + 5);
    assert!(check(&c, &index, &bm25).unwrap_err().contains("body span"));
    let mut c = good.clone();
    c.methods[3].text = Arc::from("void m3() { é }");
    c.methods[3].body = Span::new(0, 13);
    assert!(check(&c, &index, &bm25).unwrap_err().contains("UTF-8"));
    let mut c = good.clone();
    c.methods[3].body = Span::new(5, 4);
    assert!(check(&c, &index, &bm25).is_err());

    // Index / BM25 built for a different number of methods.
    let mut fewer = good.clone();
    fewer.methods.pop();
    let err = check(&fewer, &index, &Bm25::build(&fewer)).unwrap_err();
    assert!(err.contains("inverted index") && err.contains("methods"), "{err}");
    let err = check(&fewer, &InvertedIndex::build(&fewer, &AromaFeaturizer), &bm25).unwrap_err();
    assert!(err.contains("BM25"), "{err}");
}
