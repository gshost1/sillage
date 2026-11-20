//! On-disk persistence: `b"SILLAGE1"` magic, little-endian `u32` format
//! version, then `postcard((Corpus, InvertedIndex, Bm25))`.

use std::path::Path;

use anyhow::{Context, bail, ensure};

use crate::{Bm25, Corpus, InvertedIndex};

const MAGIC: &[u8; 8] = b"SILLAGE1";
/// 2: postings carry per-method feature counts (`InvertedIndex::counts`).
/// 3: BM25 terms are case-sensitive whole identifiers (`bm25::tokenize`).
pub const FORMAT_VERSION: u32 = 3;
const HEADER_LEN: usize = MAGIC.len() + 4;

pub fn save(path: &Path, corpus: &Corpus, index: &InvertedIndex, bm25: &Bm25) -> anyhow::Result<()> {
    let payload = postcard::to_stdvec(&(corpus, index, bm25)).context("serializing index")?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&payload);

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
    ensure!(
        bytes.len() >= HEADER_LEN && &bytes[..MAGIC.len()] == MAGIC,
        "{} is not a sillage index (bad magic)",
        path.display()
    );
    let version = u32::from_le_bytes(bytes[MAGIC.len()..HEADER_LEN].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        bail!(
            "{}: unsupported index format version {version} (expected {FORMAT_VERSION})",
            path.display()
        );
    }
    postcard::from_bytes(&bytes[HEADER_LEN..]).with_context(|| format!("decoding {}", path.display()))
}
