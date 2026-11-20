//! Phase I light-weight search (paper §3.3.1): a CSR inverted index over the
//! feature *sets* `S(F(m))` of every method body. `search` computes the overlap
//! score `|S(F(m)) ∩ S(F(q))|` for all methods at once (the `D · v_q` product).

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sillage_core::{FeatureBag, FeatureId, Featurizer, MethodId, SimplifiedTree};

use crate::corpus::Corpus;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvertedIndex {
    /// Sorted unique feature IDs.
    feature_ids: Vec<FeatureId>,
    /// `postings[offsets[f]..offsets[f + 1]]` are the methods containing `feature_ids[f]`.
    offsets: Vec<u32>,
    /// Method IDs, sorted ascending within each feature's range.
    postings: Vec<u32>,
    num_methods: u32,
}

impl InvertedIndex {
    /// Parses and featurizes every corpus method in parallel. Methods that fail
    /// to parse keep their ID but contribute no features.
    pub fn build(corpus: &Corpus, featurizer: &dyn Featurizer) -> Self {
        let sets: Vec<(MethodId, Vec<FeatureId>)> = corpus
            .methods
            .par_iter()
            .map(|m| {
                let set = sillage_parse::parse_method(m.lang, &m.text, m.body)
                    .map(|tree| feature_set(&tree, featurizer))
                    .unwrap_or_default();
                (m.id, set)
            })
            .collect();
        let num_methods = corpus.methods.len();
        Self::from_sets(sets, num_methods)
    }

    /// Builds from already-parsed trees. `num_methods` is raised to cover every
    /// supplied `MethodId`; repeated IDs have their feature sets unioned.
    pub fn build_from_trees(
        trees: &[(MethodId, &SimplifiedTree)],
        num_methods: usize,
        featurizer: &dyn Featurizer,
    ) -> Self {
        let sets: Vec<(MethodId, Vec<FeatureId>)> = trees
            .par_iter()
            .map(|&(id, tree)| (id, feature_set(tree, featurizer)))
            .collect();
        Self::from_sets(sets, num_methods)
    }

    fn from_sets(mut sets: Vec<(MethodId, Vec<FeatureId>)>, num_methods: usize) -> Self {
        let max_id = sets.iter().map(|(id, _)| id.0 as usize + 1).max().unwrap_or(0);
        let num_methods = u32::try_from(num_methods.max(max_id)).expect("more than u32::MAX methods");

        // Merge duplicate method IDs so postings stay strictly increasing.
        sets.par_sort_unstable_by_key(|(id, _)| *id);
        let mut merged: Vec<(MethodId, Vec<FeatureId>)> = Vec::with_capacity(sets.len());
        for (id, set) in sets {
            match merged.last_mut() {
                Some((last, acc)) if *last == id => {
                    acc.extend(set);
                    acc.sort_unstable();
                    acc.dedup();
                }
                _ => merged.push((id, set)),
            }
        }

        let mut feature_ids: Vec<FeatureId> =
            merged.iter().flat_map(|(_, s)| s.iter().copied()).collect();
        feature_ids.par_sort_unstable();
        feature_ids.dedup();

        // Dense feature index per method, computed in parallel.
        let dense: Vec<(u32, Vec<u32>)> = merged
            .into_par_iter()
            .map(|(id, set)| {
                let idx = set
                    .iter()
                    .map(|f| feature_ids.binary_search(f).expect("feature present") as u32)
                    .collect();
                (id.0, idx)
            })
            .collect();

        let mut offsets = vec![0u32; feature_ids.len() + 1];
        for (_, idx) in &dense {
            for &f in idx {
                offsets[f as usize + 1] += 1;
            }
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let total = *offsets.last().unwrap_or(&0) as usize;
        let mut cursor: Vec<u32> = offsets[..offsets.len() - 1].to_vec();
        let mut postings = vec![0u32; total];
        // `dense` is sorted by method ID, so each feature's postings end up sorted.
        for (mid, idx) in &dense {
            for &f in idx {
                let c = &mut cursor[f as usize];
                postings[*c as usize] = *mid;
                *c += 1;
            }
        }

        Self { feature_ids, offsets, postings, num_methods }
    }

    /// Top-`eta1` methods by overlap score `|S(F(m)) ∩ S(query)|`, sorted by
    /// `(overlap DESC, MethodId ASC)`. Methods with zero overlap are omitted.
    pub fn search(&self, query: &FeatureBag, eta1: usize) -> Vec<(MethodId, u32)> {
        if eta1 == 0 || query.is_empty() || self.num_methods == 0 {
            return Vec::new();
        }
        let mut counts = vec![0u16; self.num_methods as usize];
        for fid in query.set_ids() {
            if let Some(p) = self.postings_of(fid) {
                for &m in p {
                    let c = &mut counts[m as usize];
                    *c = c.saturating_add(1);
                }
            }
        }
        let mut hits: Vec<(u32, u16)> = counts
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c > 0)
            .map(|(m, &c)| (m as u32, c))
            .collect();
        let cmp = |a: &(u32, u16), b: &(u32, u16)| b.1.cmp(&a.1).then(a.0.cmp(&b.0));
        if hits.len() > eta1 {
            hits.select_nth_unstable_by(eta1, cmp);
            hits.truncate(eta1);
        }
        hits.sort_unstable_by(cmp);
        hits.into_iter().map(|(m, c)| (MethodId(m), u32::from(c))).collect()
    }

    fn postings_of(&self, fid: FeatureId) -> Option<&[u32]> {
        let f = self.feature_ids.binary_search(&fid).ok()?;
        Some(&self.postings[self.offsets[f] as usize..self.offsets[f + 1] as usize])
    }

    pub fn num_features(&self) -> usize {
        self.feature_ids.len()
    }

    pub fn num_postings(&self) -> usize {
        self.postings.len()
    }

    pub fn num_methods(&self) -> usize {
        self.num_methods as usize
    }
}

/// Sorted unique feature IDs `S(F(t))`.
fn feature_set(tree: &SimplifiedTree, featurizer: &dyn Featurizer) -> Vec<FeatureId> {
    featurizer.featurize(tree).total().set_ids().collect()
}
