//! Phase I light-weight search (paper §3.3.1): a CSR inverted index over the
//! feature *sets* `S(F(m))` of every method body. `search` computes the overlap
//! score `|S(F(m)) ∩ S(F(q))|` for all methods at once (the `D · v_q` product).
//!
//! Each posting also stores the feature's multiplicity in the method (saturated at
//! [`COUNT_SAT`]), so [`InvertedIndex::search_bounded`] can return, per candidate,
//! the multiset overlap `Σ_f min(q_f, m_f)`: an upper bound on the candidate's
//! Phase II pruned similarity that lets the engine skip re-parsing candidates that
//! cannot reach the top η2.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sillage_core::{FeatureBag, FeatureId, Featurizer, MethodId, SimplifiedTree};

use crate::corpus::Corpus;

/// Saturation value of a stored posting count: `COUNT_SAT` means "at least `COUNT_SAT`".
pub const COUNT_SAT: u8 = u8::MAX;

/// A Phase I candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub id: MethodId,
    /// Set overlap `|S(F(m)) ∩ S(F(q))|` (the Phase I score).
    pub overlap: u32,
    /// Multiset overlap `Σ_f min(q_f, m_f)` (with saturated counts treated as
    /// unbounded), an upper bound on `SimScore(F(q), F(Prune(F(q), m)))`.
    pub bound: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvertedIndex {
    /// Sorted unique feature IDs.
    feature_ids: Vec<FeatureId>,
    /// `postings[offsets[f]..offsets[f + 1]]` are the methods containing `feature_ids[f]`.
    offsets: Vec<u32>,
    /// Method IDs, sorted ascending within each feature's range.
    postings: Vec<u32>,
    /// `counts[p]` is the multiplicity of the feature in method `postings[p]`,
    /// saturated at [`COUNT_SAT`].
    counts: Vec<u8>,
    num_methods: u32,
}

/// Feature bag of one method, as `(feature, count)` sorted by feature.
pub(crate) type Bag = Vec<(FeatureId, u32)>;

impl InvertedIndex {
    /// Parses and featurizes every corpus method in parallel. Methods that fail
    /// to parse keep their ID but contribute no features.
    pub fn build(corpus: &Corpus, featurizer: &dyn Featurizer) -> Self {
        let bags: Vec<(MethodId, Bag)> = corpus
            .methods
            .par_iter()
            .map(|m| {
                let bag = sillage_parse::parse_method(m.lang, &m.text, m.body)
                    .map(|tree| feature_bag(&tree, featurizer))
                    .unwrap_or_default();
                (m.id, bag)
            })
            .collect();
        let num_methods = corpus.methods.len();
        Self::from_bags(bags, num_methods)
    }

    /// Builds from already-parsed trees. `num_methods` is raised to cover every
    /// supplied `MethodId`; repeated IDs have their feature sets unioned (counts
    /// take the maximum).
    pub fn build_from_trees(
        trees: &[(MethodId, &SimplifiedTree)],
        num_methods: usize,
        featurizer: &dyn Featurizer,
    ) -> Self {
        let bags: Vec<(MethodId, Bag)> = trees
            .par_iter()
            .map(|&(id, tree)| (id, feature_bag(tree, featurizer)))
            .collect();
        Self::from_bags(bags, num_methods)
    }

    pub(crate) fn from_bags(mut bags: Vec<(MethodId, Bag)>, num_methods: usize) -> Self {
        let max_id = bags.iter().map(|(id, _)| id.0 as usize + 1).max().unwrap_or(0);
        let num_methods = u32::try_from(num_methods.max(max_id)).expect("more than u32::MAX methods");

        // Merge duplicate method IDs so postings stay strictly increasing.
        bags.par_sort_by_key(|(id, _)| *id);
        let mut merged: Vec<(MethodId, Bag)> = Vec::with_capacity(bags.len());
        for (id, bag) in bags {
            match merged.last_mut() {
                Some((last, acc)) if *last == id => {
                    acc.extend(bag);
                    acc.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
                    acc.dedup_by_key(|e| e.0);
                }
                _ => merged.push((id, bag)),
            }
        }

        let mut feature_ids: Vec<FeatureId> =
            merged.iter().flat_map(|(_, s)| s.iter().map(|e| e.0)).collect();
        feature_ids.par_sort_unstable();
        feature_ids.dedup();

        // Dense feature index per method, computed in parallel.
        let dense: Vec<(u32, Vec<(u32, u8)>)> = merged
            .into_par_iter()
            .map(|(id, bag)| {
                let idx = bag
                    .iter()
                    .map(|&(f, c)| {
                        let d = feature_ids.binary_search(&f).expect("feature present") as u32;
                        (d, c.min(u32::from(COUNT_SAT)) as u8)
                    })
                    .collect();
                (id.0, idx)
            })
            .collect();

        let mut offsets = vec![0u32; feature_ids.len() + 1];
        for (_, idx) in &dense {
            for &(f, _) in idx {
                offsets[f as usize + 1] += 1;
            }
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let total = *offsets.last().unwrap_or(&0) as usize;
        let mut cursor: Vec<u32> = offsets[..offsets.len() - 1].to_vec();
        let mut postings = vec![0u32; total];
        let mut counts = vec![0u8; total];
        // `dense` is sorted by method ID, so each feature's postings end up sorted.
        for (mid, idx) in &dense {
            for &(f, c) in idx {
                let cur = &mut cursor[f as usize];
                postings[*cur as usize] = *mid;
                counts[*cur as usize] = c;
                *cur += 1;
            }
        }

        Self { feature_ids, offsets, postings, counts, num_methods }
    }

    /// Top-`eta1` methods by overlap score `|S(F(m)) ∩ S(query)|`, sorted by
    /// `(overlap DESC, MethodId ASC)`. Methods with zero overlap are omitted.
    pub fn search(&self, query: &FeatureBag, eta1: usize) -> Vec<(MethodId, u32)> {
        self.search_bounded(query, eta1).into_iter().map(|h| (h.id, h.overlap)).collect()
    }

    /// [`search`](Self::search), with each hit's multiset upper bound [`Hit::bound`].
    pub fn search_bounded(&self, query: &FeatureBag, eta1: usize) -> Vec<Hit> {
        if eta1 == 0 || query.is_empty() || self.num_methods == 0 {
            return Vec::new();
        }
        let n = self.num_methods as usize;
        let mut overlap = vec![0u16; n];
        let mut bound = vec![0u32; n];
        let has_counts = self.counts.len() == self.postings.len();
        for &(fid, qc) in query.entries() {
            let Some(range) = self.range_of(fid) else { continue };
            let posts = &self.postings[range.clone()];
            if has_counts {
                for (&m, &c) in posts.iter().zip(&self.counts[range]) {
                    let m = m as usize;
                    overlap[m] = overlap[m].saturating_add(1);
                    let add = if c == COUNT_SAT { qc } else { qc.min(u32::from(c)) };
                    bound[m] = bound[m].saturating_add(add);
                }
            } else {
                for &m in posts {
                    let m = m as usize;
                    overlap[m] = overlap[m].saturating_add(1);
                    bound[m] = bound[m].saturating_add(qc);
                }
            }
        }
        let mut hits: Vec<(u32, u16)> = overlap
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
        hits.into_iter()
            .map(|(m, c)| Hit { id: MethodId(m), overlap: u32::from(c), bound: u64::from(bound[m as usize]) })
            .collect()
    }

    fn range_of(&self, fid: FeatureId) -> Option<std::ops::Range<usize>> {
        let f = self.feature_ids.binary_search(&fid).ok()?;
        Some(self.offsets[f] as usize..self.offsets[f + 1] as usize)
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

/// `F(t)` as sorted `(feature, count)` pairs.
pub(crate) fn feature_bag(tree: &SimplifiedTree, featurizer: &dyn Featurizer) -> Bag {
    featurizer.featurize(tree).total().entries().to_vec()
}
