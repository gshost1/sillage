//! Stable feature hashing.
//!
//! Every feature is encoded as a canonical byte string — a one-byte family tag
//! followed by self-delimiting (type-tagged, length-prefixed) fields — and hashed
//! with `xxh3_64`. The encoding is part of the on-disk format: changing it
//! invalidates saved indexes (bump `store::FORMAT_VERSION`).

use sillage_core::FeatureId;
use xxhash_rust::xxh3::xxh3_64;

pub(crate) const TAG_TOKEN: u8 = 1;
pub(crate) const TAG_PARENT: u8 = 2;
pub(crate) const TAG_SIBLING: u8 = 3;
pub(crate) const TAG_VAR_USAGE: u8 = 4;

const FIELD_STR: u8 = b'S';
const FIELD_NUM: u8 = b'N';
pub(crate) const CTX_SLOT: u8 = b'c';
pub(crate) const CTX_MEMBER: u8 = b'm';

/// Reusable canonical encoder; `finish` hashes the bytes and clears the buffer.
#[derive(Default)]
pub(crate) struct Encoder {
    buf: Vec<u8>,
}

impl Encoder {
    pub(crate) fn tag(&mut self, tag: u8) -> &mut Self {
        self.buf.push(tag);
        self
    }

    pub(crate) fn str(&mut self, s: &str) -> &mut Self {
        self.buf.push(FIELD_STR);
        self.buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(s.as_bytes());
        self
    }

    pub(crate) fn num(&mut self, n: u16) -> &mut Self {
        self.buf.push(FIELD_NUM);
        self.buf.extend_from_slice(&n.to_le_bytes());
        self
    }

    pub(crate) fn finish(&mut self) -> FeatureId {
        let id = xxh3_64(&self.buf);
        self.buf.clear();
        id
    }
}

/// 64-bit content hash used for corpus deduplication.
pub(crate) fn content_hash(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_prefix_prevents_field_aliasing() {
        let mut e = Encoder::default();
        let a = e.tag(TAG_SIBLING).str("ab").str("c").finish();
        let b = e.tag(TAG_SIBLING).str("a").str("bc").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn tags_separate_families() {
        let mut e = Encoder::default();
        let a = e.tag(TAG_TOKEN).str("x").finish();
        let b = e.tag(TAG_PARENT).str("x").finish();
        assert_ne!(a, b);
    }

    #[test]
    fn encoder_is_deterministic_and_resets() {
        let mut e = Encoder::default();
        let a = e.tag(TAG_PARENT).str("n").num(2).str("(#)#").finish();
        let b = e.tag(TAG_PARENT).str("n").num(2).str("(#)#").finish();
        assert_eq!(a, b);
    }
}
