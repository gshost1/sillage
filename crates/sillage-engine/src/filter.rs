//! Pre-Phase-I candidate filtering ([`QueryFilter`]).
//!
//! The filter is applied inside the Phase I scan, *before* the top-η1 cut, so
//! excluded methods (the agent's own file, test code for a prod query, …)
//! never take a candidate slot from an admissible one.

use std::path::Path;

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use sillage_core::{MethodId, MethodRecord, MethodScope};
use sillage_index::Corpus;

/// Which corpus methods a query may retrieve.
///
/// `exclude_paths` entries may be relative (`src/foo.rs`, `./src/foo.rs`) or
/// absolute (`/repo/src/foo.rs`), with `/` or `\` separators. A file pattern
/// matches a method whose root-relative path ends with the pattern's path
/// segments (so `src/foo.rs` matches `repo/src/foo.rs` but not
/// `src/other_foo.rs`); an absolute pattern under the engine's root
/// ([`Engine::with_root`](crate::Engine::with_root)) is anchored at that root.
/// A pattern ending in `/` excludes a directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryFilter {
    #[serde(default)]
    pub scope: MethodScope,
    #[serde(default)]
    pub exclude_ids: Vec<MethodId>,
    #[serde(default)]
    pub exclude_paths: Vec<String>,
}

impl QueryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_scope(mut self, scope: MethodScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn prod() -> Self {
        Self::new().with_scope(MethodScope::Prod)
    }

    pub fn test() -> Self {
        Self::new().with_scope(MethodScope::Test)
    }

    pub fn exclude_id(mut self, id: MethodId) -> Self {
        self.exclude_ids.push(id);
        self
    }

    pub fn exclude_ids(mut self, ids: impl IntoIterator<Item = MethodId>) -> Self {
        self.exclude_ids.extend(ids);
        self
    }

    pub fn exclude_path(mut self, path: impl Into<String>) -> Self {
        self.exclude_paths.push(path.into());
        self
    }

    pub fn exclude_paths<S: Into<String>>(mut self, paths: impl IntoIterator<Item = S>) -> Self {
        self.exclude_paths.extend(paths.into_iter().map(Into::into));
        self
    }

    /// No restriction at all (every method is admissible).
    pub fn is_trivial(&self) -> bool {
        self.scope == MethodScope::All && self.exclude_ids.is_empty() && self.exclude_paths.is_empty()
    }

    /// Whether `exclude_paths` excludes a method at root-relative path `rel_path`.
    pub fn excludes_path(&self, rel_path: &str, root: Option<&Path>) -> bool {
        let root = root.map(|r| Pattern::new(&r.to_string_lossy()));
        let m = Pattern::new(rel_path);
        self.exclude_paths.iter().any(|p| Pattern::new(p).matches(&m.segs, root.as_ref()))
    }

    pub(crate) fn compile(&self, corpus: &Corpus, root: Option<&Path>) -> CompiledFilter {
        let ids: FxHashSet<MethodId> = self.exclude_ids.iter().copied().collect();
        let path_mask = (!self.exclude_paths.is_empty()).then(|| {
            let root = root.map(|r| Pattern::new(&r.to_string_lossy()));
            let pats: Vec<Pattern> = self.exclude_paths.iter().map(|p| Pattern::new(p)).collect();
            // Methods of one file share a verdict: memoize by (repo, path).
            let mut by_file: rustc_hash::FxHashMap<(&str, &str), bool> = rustc_hash::FxHashMap::default();
            corpus
                .methods
                .iter()
                .map(|m| {
                    let excluded = *by_file.entry((&m.repo, &m.path)).or_insert_with(|| {
                        let segs = Pattern::new(&m.rel_path()).segs;
                        pats.iter().any(|p| p.matches(&segs, root.as_ref()))
                    });
                    !excluded
                })
                .collect()
        });
        CompiledFilter { scope: self.scope, ids, path_mask }
    }
}

/// A [`QueryFilter`] resolved against one corpus.
#[derive(Debug, Clone)]
pub(crate) struct CompiledFilter {
    scope: MethodScope,
    ids: FxHashSet<MethodId>,
    /// `path_mask[i]`: method `i` survives `exclude_paths`.
    path_mask: Option<Vec<bool>>,
}

impl CompiledFilter {
    pub(crate) fn admits(&self, m: &MethodRecord) -> bool {
        self.scope.admits(m.is_test)
            && !self.ids.contains(&m.id)
            && self.path_mask.as_ref().is_none_or(|mask| mask.get(m.id.0 as usize).copied().unwrap_or(true))
    }
}

/// A normalized path: segments without empty / `.` parts, `..` resolved
/// where possible.
#[derive(Debug, Clone)]
struct Pattern {
    segs: Vec<String>,
    absolute: bool,
    dir: bool,
}

impl Pattern {
    fn new(p: &str) -> Self {
        let p = p.trim().replace('\\', "/");
        let drive = p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic();
        let absolute = p.starts_with('/') || drive;
        let dir = p.ends_with('/') && p.len() > 1;
        let mut segs: Vec<String> = Vec::new();
        for s in p.split('/') {
            match s {
                "" | "." => {}
                ".." if segs.last().is_some_and(|l| l != "..") => {
                    segs.pop();
                }
                s => segs.push(s.to_string()),
            }
        }
        Self { segs, absolute, dir }
    }

    /// Does this pattern exclude a method at root-relative segments `m`?
    fn matches(&self, m: &[String], root: Option<&Pattern>) -> bool {
        if self.segs.is_empty() || m.is_empty() {
            return false;
        }
        // Absolute pattern under the engine root: anchored at the root.
        if self.absolute
            && let Some(root) = root.filter(|r| r.absolute && !r.segs.is_empty())
            && self.segs.len() >= root.segs.len()
            && self.segs[..root.segs.len()] == root.segs[..]
        {
            let rel = &self.segs[root.segs.len()..];
            if rel.is_empty() {
                return true; // the root itself
            }
            return if self.dir { m.starts_with(rel) && m.len() > rel.len() } else { m == rel || m.starts_with(rel) && m.len() > rel.len() };
        }
        let p = &self.segs[..];
        if self.dir {
            // A directory anywhere in the path (`src/gen/` in `repo/src/gen/x.rs`).
            return m.len() > p.len() && m[..m.len() - 1].windows(p.len()).any(|w| w == p);
        }
        // File: segment suffix either way (an absolute path to the same file in
        // an unknown root ends with the method's relative path).
        m.ends_with(p) || (self.absolute && p.ends_with(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(pattern: &str, rel: &str, root: Option<&str>) -> bool {
        QueryFilter::new().exclude_path(pattern).excludes_path(rel, root.map(Path::new))
    }

    #[test]
    fn path_patterns_match_by_segment_suffix() {
        assert!(ex("src/foo.rs", "src/foo.rs", None));
        assert!(ex("./src/foo.rs", "src/foo.rs", None));
        assert!(ex("src/foo.rs", "repo/src/foo.rs", None));
        assert!(ex("foo.rs", "repo/src/foo.rs", None));
        assert!(ex("src\\foo.rs", "src/foo.rs", None));
        assert!(ex("src//./foo.rs", "src/foo.rs", None));
        assert!(ex("src/x/../foo.rs", "src/foo.rs", None));
        assert!(!ex("src/foo.rs", "src/other_foo.rs", None));
        assert!(!ex("oo.rs", "src/foo.rs", None));
        assert!(!ex("src/foo.rs", "lib/foo.rs", None));
        assert!(!ex("src/foo.rs", "src/foo.rs.bak", None));
        assert!(!ex("", "src/foo.rs", None));
        // Absolute without a known root: the method path is a suffix of it.
        assert!(ex("/home/u/repo/src/foo.rs", "src/foo.rs", None));
        assert!(!ex("/home/u/repo/src/foo.rs", "src/bar.rs", None));
    }

    #[test]
    fn absolute_patterns_anchor_at_the_root() {
        let root = Some("/home/u/repo");
        assert!(ex("/home/u/repo/src/foo.rs", "src/foo.rs", root));
        assert!(ex("/home/u/repo/./src/foo.rs", "src/foo.rs", root));
        assert!(!ex("/home/u/repo/src/foo.rs", "lib/src/foo.rs", root), "anchored, not a suffix match");
        assert!(ex("/home/u/repo/src/", "src/a/b.rs", root));
        assert!(ex("/home/u/repo/src", "src/a/b.rs", root));
        assert!(!ex("/home/u/repo/src/", "srcx/b.rs", root));
        // Outside the root: falls back to suffix matching.
        assert!(ex("/elsewhere/repo/src/foo.rs", "src/foo.rs", root));
    }

    #[test]
    fn directory_patterns() {
        assert!(ex("src/gen/", "repo/src/gen/x.rs", None));
        assert!(ex("gen/", "src/gen/x.rs", None));
        assert!(!ex("gen/", "src/generated/x.rs", None));
        assert!(!ex("gen/", "src/gen", None), "a file named like the directory");
    }

    #[test]
    fn builders_and_triviality() {
        assert!(QueryFilter::default().is_trivial());
        let f = QueryFilter::prod().exclude_id(MethodId(3)).exclude_ids([MethodId(4)]).exclude_paths(["a.rs", "b.rs"]);
        assert_eq!(f.scope, MethodScope::Prod);
        assert_eq!(f.exclude_ids, [MethodId(3), MethodId(4)]);
        assert_eq!(f.exclude_paths, ["a.rs", "b.rs"]);
        assert!(!f.is_trivial());
        assert_eq!(QueryFilter::test().scope, MethodScope::Test);
    }
}
