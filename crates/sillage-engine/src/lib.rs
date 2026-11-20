//! Sillage engine: Aroma's prune (Algorithm 1), rerank (Phase II), cluster and
//! intersect (Phase III), and the end-to-end [`Engine`] pipeline.

mod cluster;
mod complete;
mod intersect;
mod pipeline;
mod prune;
mod rerank;

pub use cluster::{ScoredCluster, cluster, cluster_scored};
pub use complete::complete_calls;
pub use intersect::intersect;
pub use prune::{prune, prune_keep};
pub use rerank::{TieKey, no_full_bound, rerank, rerank_top};

use serde::{Deserialize, Serialize};
use sillage_core::{Featurizer, MethodId, Params, SimplifiedTree};
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

pub struct Engine {
    pub corpus: Corpus,
    pub index: InvertedIndex,
    pub bm25: Bm25,
    pub featurizer: Box<dyn Featurizer>,
    pub params: Params,
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
        }
    }
}
