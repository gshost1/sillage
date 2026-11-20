use serde::{Deserialize, Serialize};
use sillage_core::{FeatureBag, Featurizer, Lang, LeafFeatures, MethodId, Params, SimplifiedTree};
use sillage_index::{Bm25, Corpus, InvertedIndex};

#[derive(Debug, Clone)]
pub struct Pruned {
    pub keep: Vec<bool>,
    pub sim: u64,
    pub tree: SimplifiedTree,
}

pub fn prune(_fq: &FeatureBag, _m: &SimplifiedTree, _lf: &LeafFeatures) -> Pruned {
    todo!("Slice C")
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

pub fn rerank(
    _fq: &FeatureBag,
    _cands: Vec<(MethodId, u32, SimplifiedTree)>,
    _featurizer: &dyn Featurizer,
) -> Vec<Ranked> {
    todo!("Slice C")
}

pub fn cluster(
    _fq: &FeatureBag,
    _n2: &[Ranked],
    _params: &Params,
    _featurizer: &dyn Featurizer,
) -> Vec<Vec<usize>> {
    todo!("Slice C")
}

pub fn intersect(
    _tuple: &[usize],
    _fq: &FeatureBag,
    _n2: &[Ranked],
    _params: &Params,
    _featurizer: &dyn Featurizer,
) -> SimplifiedTree {
    todo!("Slice C")
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

    pub fn search(&self, _lang: Lang, _snippet: &str, _n: usize) -> anyhow::Result<Vec<Ranked>> {
        todo!("Slice C")
    }

    pub fn features_only(
        &self,
        _lang: Lang,
        _snippet: &str,
        _n: usize,
    ) -> anyhow::Result<Vec<MethodId>> {
        todo!("Slice C")
    }

    pub fn bm25_search(&self, _snippet: &str, _n: usize) -> Vec<MethodId> {
        todo!("Slice C")
    }

    pub fn recommend(&self, _lang: Lang, _snippet: &str) -> anyhow::Result<Output> {
        todo!("Slice C")
    }
}
