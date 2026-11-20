//! Sillage engine: Aroma's prune (Algorithm 1), rerank (Phase II), cluster and
//! intersect (Phase III), and the end-to-end [`Engine`] pipeline.

mod agent;
mod align;
mod cluster;
mod complete;
mod filter;
mod hunks;
mod intersect;
mod pipeline;
mod prune;
mod rerank;

pub use cluster::{ScoredCluster, cluster, cluster_scored};
pub use agent::{
    Citation, Confidence, GuardKind, GuardOptions, HunkGuards, Idiom, IdiomOptions, MissingGuard, TestIdiom,
    TestIdiomOptions, TestIdioms, TestLink, classify_guard,
};
pub use align::{AlignedRecommendation, Rename, VarAlignment, align_variables, render_aligned};
pub use complete::complete_calls;
pub use filter::QueryFilter;
pub use hunks::{DiffHunk, DiffLine, extract_diff_hunks};
pub use intersect::intersect;
pub use prune::{prune, prune_keep};
pub use rerank::{TieKey, no_full_bound, rerank, rerank_top};

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sillage_core::{Featurizer, Lang, MethodId, Params, SimplifiedTree};
use sillage_index::{Bm25, Corpus, InvertedIndex};

#[derive(Debug, Clone)]
pub struct Pruned {
    pub keep: Vec<bool>,
    pub sim: u64,
    pub tree: SimplifiedTree,
}

#[derive(Debug, Clone)]
pub struct Ranked {
    pub id: MethodId,
    /// Phase I set overlap `|S(F(q)) ∩ S(F(m))|`.
    pub overlap: u32,
    /// Pruned similarity (the Phase II score's numerator).
    pub sim: u64,
    /// Tie-breaker: `|F_full(q) ∩ F(m)|`, the multiset overlap of the method with
    /// the query's *full* featurization (cross-line features included).
    pub full_sim: u64,
    /// Tie-breaker: `|F(m)|`, the method's feature count.
    pub size: u64,
    pub score: f64,
    pub pruned: Pruned,
    pub tree: SimplifiedTree,
}

impl Ranked {
    /// This entry's Phase II ranking key.
    pub fn key(&self) -> TieKey {
        TieKey { sim: self.sim, overlap: self.overlap, full_sim: self.full_sim, size: self.size, id: self.id }
    }
}

#[derive(Debug, Clone)]
pub struct Recommendation {
    pub cluster: Vec<MethodId>,
    pub tree: SimplifiedTree,
    pub query_match: Vec<bool>,
    pub l: f64,
    pub s: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Timings {
    /// Query snippet parse + featurize.
    pub parse_ms: f64,
    /// Phase I: inverted-index search (with multiset bounds).
    pub phase1_ms: f64,
    /// Phase II: bounded re-parse, prune and rerank of the candidates.
    pub phase2_ms: f64,
    /// Phase III: cluster and intersect.
    pub phase3_ms: f64,
    pub total_ms: f64,
    /// Phase I candidates re-parsed by the bounded Phase II (at most η1).
    #[serde(default)]
    pub parsed: usize,
}

#[derive(Debug, Clone)]
pub struct Output {
    /// The reranked candidates Phase III can use: the top η2 of N2 with
    /// score > τ1 (lower-ranked candidates are never re-parsed).
    pub n2: Vec<Ranked>,
    pub recommendations: Vec<Recommendation>,
    pub timings: Timings,
}

/// One structural search hit with everything an agent needs to cite it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarMatch {
    pub id: MethodId,
    pub name: String,
    /// Path relative to the index root.
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    /// `path:line_start`.
    pub provenance: String,
    pub lang: Lang,
    pub is_test: bool,
    /// Phase II score `SimScore(q, Prune(q, m)) / |F(q)|` in `[0, 1]`.
    pub score: f64,
    pub sim: u64,
    pub overlap: u32,
    /// The method's source text.
    pub snippet: String,
}

pub struct Engine {
    pub corpus: Corpus,
    pub index: InvertedIndex,
    pub bm25: Bm25,
    pub featurizer: Box<dyn Featurizer>,
    pub params: Params,
    /// Absolute directory the corpus was indexed from, used to anchor absolute
    /// [`QueryFilter::exclude_paths`] (see [`Engine::with_root`]).
    pub root: Option<PathBuf>,
}

impl Engine {
    pub fn new(
        corpus: Corpus,
        index: InvertedIndex,
        bm25: Bm25,
        featurizer: Box<dyn Featurizer>,
        params: Params,
    ) -> Self {
        Self {
            corpus,
            index,
            bm25,
            featurizer,
            params,
            root: None,
        }
    }

    /// Records the absolute directory the corpus was indexed from, so absolute
    /// excluded paths are matched relative to it.
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        self.root = Some(std::fs::canonicalize(&root).unwrap_or(root));
        self
    }

    /// An engine over freshly built index parts.
    pub fn from_parts(parts: sillage_index::IndexParts, featurizer: Box<dyn Featurizer>, params: Params) -> Self {
        Self::new(parts.corpus, parts.index, parts.bm25, featurizer, params)
    }

    /// Swaps in a rebuilt index (e.g. from [`sillage_index::IncrementalIndex::refresh`]),
    /// keeping the featurizer, parameters and root.
    pub fn replace_index(&mut self, parts: sillage_index::IndexParts) {
        self.corpus = parts.corpus;
        self.index = parts.index;
        self.bm25 = parts.bm25;
    }
}
