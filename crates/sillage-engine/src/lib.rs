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
pub use prune::prune;
pub use rerank::rerank;

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
    pub overlap: u32,
    pub sim: u64,
    pub score: f64,
    pub pruned: Pruned,
    pub tree: SimplifiedTree,
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
    pub parse_ms: f64,
    pub phase1_ms: f64,
    pub phase2_ms: f64,
    pub phase3_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone)]
pub struct Output {
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
