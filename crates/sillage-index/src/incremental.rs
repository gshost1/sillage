//! Incremental directory indexing for long-lived processes (an MCP server
//! watching the repository an agent edits).
//!
//! [`IncrementalIndex`] caches, per source file, its metadata (size, mtime),
//! content hash and extraction result (methods, imports, test flags, feature
//! bags). [`IncrementalIndex::refresh`] re-walks the root, re-reads only files
//! whose size or mtime changed (or whose mtime is too recent to trust),
//! re-parses only files whose content hash changed, drops removed files, and
//! rebuilds the corpus, inverted index and BM25 from the cache. The rebuilt
//! index is identical to a from-scratch [`index_dir`](crate::index_dir) over
//! the same files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sillage_core::{Featurizer, Lang};

use crate::corpus::{FileUnit, IndexParts, build_parts, extract_unit, split_repo, walk_with_meta};
use crate::hash::content_hash;
use crate::index::{Bag, feature_bag};

/// An mtime this close to (or after) the previous scan may hide a same-size
/// edit made within the filesystem's timestamp granularity, so such files are
/// re-hashed even when their metadata looks unchanged.
const RACY_WINDOW: Duration = Duration::from_secs(2);

/// What one [`IncrementalIndex::refresh`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct RefreshStats {
    pub files_added: usize,
    pub files_modified: usize,
    pub files_removed: usize,
    pub files_unchanged: usize,
    /// Source files tracked after the refresh.
    pub total_files: usize,
    /// Methods in the current corpus (after deduplication).
    pub total_methods: usize,
    /// Files whose content was re-read and hashed (changed metadata or racy mtime).
    pub files_rehashed: usize,
    pub walk_errors: usize,
    pub read_errors: usize,
    pub skipped_large: usize,
    pub elapsed_ms: f64,
}

impl RefreshStats {
    /// Whether any file was added, modified or removed.
    pub fn changed(&self) -> bool {
        self.files_added + self.files_modified + self.files_removed > 0
    }
}

/// The result of a refresh: stats, and the rebuilt index when anything changed
/// (always on the first refresh).
#[derive(Debug)]
pub struct Refresh {
    pub stats: RefreshStats,
    pub parts: Option<IndexParts>,
}

struct Cached {
    lang: Lang,
    len: u64,
    mtime: Option<SystemTime>,
    unit: FileUnit<Bag>,
}

/// File-level cache over a directory, see the [module docs](self).
pub struct IncrementalIndex {
    root: PathBuf,
    langs: Vec<Lang>,
    featurizer: Box<dyn Featurizer>,
    files: BTreeMap<PathBuf, Cached>,
    last_scan: Option<SystemTime>,
    total_methods: usize,
    built: bool,
}

impl IncrementalIndex {
    /// Tracks `root` for `langs`; nothing is read until [`refresh`](Self::refresh).
    pub fn new(root: impl Into<PathBuf>, langs: &[Lang], featurizer: Box<dyn Featurizer>) -> anyhow::Result<Self> {
        let root = root.into();
        crate::corpus::check_root(&root)?;
        Ok(Self {
            root,
            langs: langs.to_vec(),
            featurizer,
            files: BTreeMap::new(),
            last_scan: None,
            total_methods: 0,
            built: false,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn langs(&self) -> &[Lang] {
        &self.langs
    }

    /// Number of source files currently tracked.
    pub fn num_files(&self) -> usize {
        self.files.len()
    }

    /// Re-scans the root and returns the rebuilt index when anything changed.
    pub fn refresh(&mut self) -> anyhow::Result<Refresh> {
        let start = Instant::now();
        let scan_time = SystemTime::now();
        let (walked, cstats) = walk_with_meta(&self.root, &self.langs)?;
        let mut stats = RefreshStats {
            walk_errors: cstats.walk_errors,
            skipped_large: cstats.skipped_large,
            ..RefreshStats::default()
        };
        let racy_after = self.last_scan.map(|t| t.checked_sub(RACY_WINDOW).unwrap_or(t));

        // Classify by metadata; files that need reading are hashed (and, when
        // their content changed, re-extracted) in parallel.
        let mut present: Vec<PathBuf> = Vec::with_capacity(walked.len());
        let mut to_read: Vec<(PathBuf, Lang, u64, Option<SystemTime>)> = Vec::new();
        for (abs, lang, meta) in walked {
            let mtime = meta.modified().ok();
            let trusted = match (self.files.get(&abs), mtime, racy_after) {
                (Some(c), Some(m), Some(racy)) => c.lang == lang && c.len == meta.len() && c.mtime == Some(m) && m < racy,
                _ => false,
            };
            if trusted {
                stats.files_unchanged += 1;
            } else {
                to_read.push((abs.clone(), lang, meta.len(), mtime));
            }
            present.push(abs);
        }
        stats.files_rehashed = to_read.len();

        let root = &self.root;
        let featurizer = self.featurizer.as_ref();
        let cache = &self.files;
        enum Outcome {
            Unchanged,
            Changed(FileUnit<Bag>),
            Unreadable,
        }
        let outcomes: Vec<(PathBuf, Lang, u64, Option<SystemTime>, Outcome)> = to_read
            .into_par_iter()
            .map(|(abs, lang, len, mtime)| {
                let outcome = match (std::fs::read_to_string(&abs), abs.strip_prefix(root).ok().and_then(split_repo)) {
                    (Ok(src), Some((repo, path))) => {
                        let same = cache.get(&abs).is_some_and(|c| c.lang == lang && c.unit.hash == content_hash(src.as_bytes()));
                        if same {
                            Outcome::Unchanged
                        } else {
                            Outcome::Changed(extract_unit(repo, path, lang, &src, &|m| feature_bag(&m.tree, featurizer)))
                        }
                    }
                    _ => Outcome::Unreadable,
                };
                (abs, lang, len, mtime, outcome)
            })
            .collect();

        for (abs, lang, len, mtime, outcome) in outcomes {
            match outcome {
                Outcome::Unchanged => {
                    stats.files_unchanged += 1;
                    if let Some(c) = self.files.get_mut(&abs) {
                        c.len = len;
                        c.mtime = mtime;
                    }
                }
                Outcome::Changed(unit) => {
                    let cached = Cached { lang, len, mtime, unit };
                    if self.files.insert(abs, cached).is_some() {
                        stats.files_modified += 1;
                    } else {
                        stats.files_added += 1;
                    }
                }
                Outcome::Unreadable => {
                    stats.read_errors += 1;
                    if self.files.remove(&abs).is_some() {
                        stats.files_removed += 1;
                    }
                }
            }
        }

        let present: std::collections::BTreeSet<PathBuf> = present.into_iter().collect();
        let before = self.files.len();
        self.files.retain(|p, _| present.contains(p));
        stats.files_removed += before - self.files.len();

        let parts = if stats.changed() || !self.built {
            let parts = self.build();
            self.total_methods = parts.corpus.methods.len();
            self.built = true;
            Some(parts)
        } else {
            None
        };
        self.last_scan = Some(scan_time);
        stats.total_files = self.files.len();
        stats.total_methods = self.total_methods;
        stats.elapsed_ms = start.elapsed().as_secs_f64() * 1e3;
        Ok(Refresh { stats, parts })
    }

    /// Builds the corpus, index and BM25 from the cache (no file access).
    pub fn build(&self) -> IndexParts {
        let mut units: Vec<&FileUnit<Bag>> = self.files.values().map(|c| &c.unit).collect();
        units.sort_by(|a, b| (&a.repo, &a.path).cmp(&(&b.repo, &b.path)));
        build_parts(&units)
    }
}
