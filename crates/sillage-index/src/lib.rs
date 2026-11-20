use std::path::Path;

use serde::{Deserialize, Serialize};
use sillage_core::{FeatureBag, Featurizer, Lang, LeafFeatures, MethodId, MethodRecord, SimplifiedTree};

#[derive(Debug, Clone, Default)]
pub struct AromaFeaturizer;

impl Featurizer for AromaFeaturizer {
    fn featurize(&self, _tree: &SimplifiedTree) -> LeafFeatures {
        todo!("Slice B")
    }
}

pub fn feature_debug(_tree: &SimplifiedTree, _leaf: usize) -> Vec<String> {
    todo!("Slice B")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Corpus {
    pub methods: Vec<MethodRecord>,
}

pub fn build_corpus(_root: &Path, _langs: &[Lang]) -> Corpus {
    todo!("Slice B")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InvertedIndex {
    _private: (),
}

impl InvertedIndex {
    pub fn build(_corpus: &Corpus, _featurizer: &dyn Featurizer) -> Self {
        todo!("Slice B")
    }

    pub fn search(&self, _query: &FeatureBag, _eta1: usize) -> Vec<(MethodId, u32)> {
        todo!("Slice B")
    }

    pub fn num_features(&self) -> usize {
        todo!("Slice B")
    }

    pub fn num_postings(&self) -> usize {
        todo!("Slice B")
    }

    pub fn num_methods(&self) -> usize {
        todo!("Slice B")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bm25 {
    _private: (),
}

impl Bm25 {
    pub fn build(_corpus: &Corpus) -> Self {
        todo!("Slice B")
    }

    pub fn search(&self, _query: &str, _n: usize) -> Vec<(MethodId, f64)> {
        todo!("Slice B")
    }
}

pub fn save(
    _path: &Path,
    _corpus: &Corpus,
    _index: &InvertedIndex,
    _bm25: &Bm25,
) -> anyhow::Result<()> {
    todo!("Slice B")
}

pub fn load(_path: &Path) -> anyhow::Result<(Corpus, InvertedIndex, Bm25)> {
    todo!("Slice B")
}
