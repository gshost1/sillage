use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::tree::SimplifiedTree;

/// 64-bit stable xxh3 feature identifier.
pub type FeatureId = u64;

/// Sorted sparse multiset of `(FeatureId, count)` entries with strictly positive counts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureBag(Vec<(FeatureId, u32)>);

impl FeatureBag {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn entries(&self) -> &[(FeatureId, u32)] {
        &self.0
    }

    pub fn from_unsorted(iter: impl IntoIterator<Item = FeatureId>) -> Self {
        let mut ids: Vec<FeatureId> = iter.into_iter().collect();
        if ids.is_empty() {
            return Self::new();
        }
        ids.sort_unstable();
        let mut out = Vec::with_capacity(ids.len());
        let mut cur = ids[0];
        let mut count: u32 = 1;
        for id in ids.into_iter().skip(1) {
            if id == cur {
                count = count.saturating_add(1);
            } else {
                out.push((cur, count));
                cur = id;
                count = 1;
            }
        }
        out.push((cur, count));
        Self(out)
    }

    pub fn from_weighted(iter: impl IntoIterator<Item = (FeatureId, u32)>) -> Self {
        let mut pairs: Vec<(FeatureId, u32)> = iter.into_iter().filter(|&(_, c)| c > 0).collect();
        if pairs.is_empty() {
            return Self::new();
        }
        pairs.sort_unstable_by_key(|&(id, _)| id);
        let mut out = Vec::with_capacity(pairs.len());
        let mut cur = pairs[0].0;
        let mut count = pairs[0].1;
        for (id, c) in pairs.into_iter().skip(1) {
            if id == cur {
                count = count.saturating_add(c);
            } else {
                out.push((cur, count));
                cur = id;
                count = c;
            }
        }
        out.push((cur, count));
        Self(out)
    }

    /// Multiset cardinality `|X| = Σ count`.
    pub fn total(&self) -> u64 {
        self.0.iter().map(|&(_, c)| u64::from(c)).sum()
    }

    /// Unique feature count `|S(X)|`.
    pub fn set_len(&self) -> usize {
        self.0.len()
    }

    /// Sorted iterator over unique feature IDs `S(X)`.
    pub fn set_ids(&self) -> impl Iterator<Item = FeatureId> + '_ {
        self.0.iter().map(|&(id, _)| id)
    }

    /// Lookup multiplicity of a single feature ID.
    pub fn get(&self, id: FeatureId) -> u32 {
        self.0
            .binary_search_by_key(&id, |&(k, _)| k)
            .map(|idx| self.0[idx].1)
            .unwrap_or(0)
    }

    /// Multiset sum (`⊎`): adds counts of matching features.
    pub fn sum(&self, other: &Self) -> Self {
        let mut out = Vec::with_capacity(self.0.len() + other.0.len());
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            let (a_id, a_c) = self.0[i];
            let (b_id, b_c) = other.0[j];
            match a_id.cmp(&b_id) {
                Ordering::Less => {
                    out.push((a_id, a_c));
                    i += 1;
                }
                Ordering::Greater => {
                    out.push((b_id, b_c));
                    j += 1;
                }
                Ordering::Equal => {
                    out.push((a_id, a_c.saturating_add(b_c)));
                    i += 1;
                    j += 1;
                }
            }
        }
        out.extend_from_slice(&self.0[i..]);
        out.extend_from_slice(&other.0[j..]);
        Self(out)
    }

    /// Multiset union (`∪`): takes elementwise `max` of counts.
    pub fn union(&self, other: &Self) -> Self {
        let mut out = Vec::with_capacity(self.0.len() + other.0.len());
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            let (a_id, a_c) = self.0[i];
            let (b_id, b_c) = other.0[j];
            match a_id.cmp(&b_id) {
                Ordering::Less => {
                    out.push((a_id, a_c));
                    i += 1;
                }
                Ordering::Greater => {
                    out.push((b_id, b_c));
                    j += 1;
                }
                Ordering::Equal => {
                    out.push((a_id, a_c.max(b_c)));
                    i += 1;
                    j += 1;
                }
            }
        }
        out.extend_from_slice(&self.0[i..]);
        out.extend_from_slice(&other.0[j..]);
        Self(out)
    }

    /// Multiset intersection (`∩`): takes elementwise `min` of counts.
    pub fn intersect(&self, other: &Self) -> Self {
        let mut out = Vec::with_capacity(self.0.len().min(other.0.len()));
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            let (a_id, a_c) = self.0[i];
            let (b_id, b_c) = other.0[j];
            match a_id.cmp(&b_id) {
                Ordering::Less => i += 1,
                Ordering::Greater => j += 1,
                Ordering::Equal => {
                    let c = a_c.min(b_c);
                    if c > 0 {
                        out.push((a_id, c));
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        Self(out)
    }

    /// Multiset intersection cardinality (`SimScore = |A ∩ B| = Σ min(a_f, b_f)`).
    pub fn intersect_count(&self, other: &Self) -> u64 {
        let mut total: u64 = 0;
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            let (a_id, a_c) = self.0[i];
            let (b_id, b_c) = other.0[j];
            match a_id.cmp(&b_id) {
                Ordering::Less => i += 1,
                Ordering::Greater => j += 1,
                Ordering::Equal => {
                    total += u64::from(a_c.min(b_c));
                    i += 1;
                    j += 1;
                }
            }
        }
        total
    }

    /// Set intersection cardinality `|S(A) ∩ S(B)|` (Phase I overlap score).
    pub fn set_intersect_count(&self, other: &Self) -> u32 {
        let mut total: u32 = 0;
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            match self.0[i].0.cmp(&other.0[j].0) {
                Ordering::Less => i += 1,
                Ordering::Greater => j += 1,
                Ordering::Equal => {
                    total += 1;
                    i += 1;
                    j += 1;
                }
            }
        }
        total
    }
}

/// Per-leaf feature bags aligned 1-to-1 with `tree.leaves`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafFeatures {
    pub per_leaf: Vec<FeatureBag>,
}

impl LeafFeatures {
    /// Total tree feature multiset `F(t) = ⊎_{n ∈ N(t)} F(n)`.
    pub fn total(&self) -> FeatureBag {
        let total_entries: usize = self.per_leaf.iter().map(|b| b.entries().len()).sum();
        let mut pairs = Vec::with_capacity(total_entries);
        for bag in &self.per_leaf {
            pairs.extend_from_slice(bag.entries());
        }
        FeatureBag::from_weighted(pairs)
    }
}

pub trait Featurizer: Send + Sync {
    fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures;

    /// Features of a partial query snippet (`F(q)`). Defaults to
    /// [`featurize`](Self::featurize); a featurizer may drop features that a
    /// snippet cannot share with the method it was taken from.
    fn featurize_query(&self, tree: &SimplifiedTree) -> LeafFeatures {
        self.featurize(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn multiset_algebra_laws(
            a in prop::collection::vec(0u64..32, 0..64),
            b in prop::collection::vec(0u64..32, 0..64),
            c in prop::collection::vec(0u64..32, 0..64),
        ) {
            let ba = FeatureBag::from_unsorted(a);
            let bb = FeatureBag::from_unsorted(b);
            let bc = FeatureBag::from_unsorted(c);

            // Commutativity
            prop_assert_eq!(ba.sum(&bb), bb.sum(&ba));
            prop_assert_eq!(ba.union(&bb), bb.union(&ba));
            prop_assert_eq!(ba.intersect(&bb), bb.intersect(&ba));
            prop_assert_eq!(ba.intersect_count(&bb), bb.intersect_count(&ba));

            // Associativity
            prop_assert_eq!(ba.sum(&bb).sum(&bc), ba.sum(&bb.sum(&bc)));
            prop_assert_eq!(ba.union(&bb).union(&bc), ba.union(&bb.union(&bc)));
            prop_assert_eq!(ba.intersect(&bb).intersect(&bc), ba.intersect(&bb.intersect(&bc)));

            // Cardinality identities
            prop_assert_eq!(ba.sum(&bb).total(), ba.total() + bb.total());
            prop_assert_eq!(ba.intersect(&bb).total(), ba.intersect_count(&bb));
            prop_assert_eq!(ba.union(&bb).total() + ba.intersect(&bb).total(), ba.total() + bb.total());
            prop_assert!(ba.set_intersect_count(&bb) as u64 <= ba.intersect_count(&bb));
        }
    }
}
