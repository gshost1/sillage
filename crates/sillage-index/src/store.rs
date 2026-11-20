//! On-disk persistence.
//!
//! Layout (format version 6):
//!
//! | bytes   | content                                              |
//! |---------|------------------------------------------------------|
//! | 0..8    | magic `b"SILLAGE1"`                                  |
//! | 8..12   | format version, little-endian `u32`                  |
//! | 12..20  | `xxh3_64` of the payload, little-endian `u64`        |
//! | 20..    | payload: `postcard((Corpus, InvertedIndex, Bm25))`   |
//!
//! [`load`] checks the magic, the version and the checksum *before* decoding,
//! then validates every structural invariant the query path relies on
//! ([`Corpus::validate`], [`InvertedIndex::validate`], [`Bm25::validate`]), so a
//! truncated, corrupted or hand-edited file is rejected with an error instead of
//! panicking (or reading out of bounds) at query time.

use std::path::Path;

use anyhow::{Context, bail, ensure};
use xxhash_rust::xxh3::xxh3_64;

use crate::{Bm25, Corpus, InvertedIndex};

const MAGIC: &[u8; 8] = b"SILLAGE1";
/// 2: postings carry per-method feature counts (`InvertedIndex::counts`).
/// 3: BM25 terms are case-sensitive whole identifiers (`bm25::tokenize`).
/// 4: `xxh3_64` payload checksum in the header; structures validated on load.
/// 5: per-method feature counts `|F(m)|` (`InvertedIndex::sizes`) for the Phase II tie-break bound.
/// 6: per-method `is_test` and `file_id`, and the per-file import table `Corpus::file_imports`.
pub const FORMAT_VERSION: u32 = 6;
const HEADER_LEN: usize = MAGIC.len() + 4 + 8;

/// Serializes the index to bytes (header + payload).
pub fn to_bytes(corpus: &Corpus, index: &InvertedIndex, bm25: &Bm25) -> anyhow::Result<Vec<u8>> {
    let payload = postcard::to_stdvec(&(corpus, index, bm25)).context("serializing index")?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&xxh3_64(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// Decodes and validates bytes produced by [`to_bytes`]; `what` names the
/// source in error messages.
pub fn from_bytes(bytes: &[u8], what: &str) -> anyhow::Result<(Corpus, InvertedIndex, Bm25)> {
    ensure!(
        bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC,
        "{what} is not a sillage index (bad magic)"
    );
    ensure!(bytes.len() >= MAGIC.len() + 4, "{what}: truncated index header");
    let version = u32::from_le_bytes(bytes[MAGIC.len()..MAGIC.len() + 4].try_into().expect("4 bytes"));
    if version < FORMAT_VERSION {
        bail!(
            "{what}: index format version {version} is older than the supported version {FORMAT_VERSION}; \
             rebuild it with `sillage index`"
        );
    }
    if version != FORMAT_VERSION {
        bail!("{what}: unsupported index format version {version} (expected {FORMAT_VERSION})");
    }
    ensure!(bytes.len() >= HEADER_LEN, "{what}: truncated index header");
    let stored = u64::from_le_bytes(bytes[MAGIC.len() + 4..HEADER_LEN].try_into().expect("8 bytes"));
    let payload = &bytes[HEADER_LEN..];
    let actual = xxh3_64(payload);
    ensure!(
        stored == actual,
        "{what}: index checksum mismatch (stored {stored:016x}, computed {actual:016x}); \
         the file is corrupted or truncated, rebuild it with `sillage index`"
    );
    let (corpus, index, bm25): (Corpus, InvertedIndex, Bm25) =
        postcard::from_bytes(payload).with_context(|| format!("decoding {what}"))?;
    let n = corpus.methods.len();
    corpus.validate().with_context(|| format!("{what}: invalid corpus"))?;
    index.validate(n).with_context(|| format!("{what}: invalid inverted index"))?;
    bm25.validate(n).with_context(|| format!("{what}: invalid BM25 index"))?;
    Ok((corpus, index, bm25))
}

pub fn save(path: &Path, corpus: &Corpus, index: &InvertedIndex, bm25: &Bm25) -> anyhow::Result<()> {
    let bytes = to_bytes(corpus, index, bm25)?;
    // Write to a sibling temp file and rename so a crash never leaves a torn index.
    let mut tmp_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

pub fn load(path: &Path) -> anyhow::Result<(Corpus, InvertedIndex, Bm25)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    from_bytes(&bytes, &path.display().to_string())
}
