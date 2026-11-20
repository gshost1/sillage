//! Algorithm 1 (Paper §3.3.2): greedy leaf selection maximising `SimScore(F(m1), F)`.
//!
//! The marginal gain of a leaf `n` given the running multiset `F`,
//! `Σ_f [min(q_f, F_f + n_f) − min(q_f, F_f)]`, is non-increasing as `F` grows
//! (capacitated coverage is submodular). A lazy-greedy (CELF) max-heap therefore
//! yields exactly the same picks as the naive `O(|N|²)` loop, including the
//! earliest-index tie-break.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rustc_hash::FxHashMap;
use sillage_core::{FeatureBag, LeafFeatures, SimplifiedTree};

use crate::Pruned;

/// Per-leaf features restricted to the query's support, as `(dense query index, count)`.
struct Projected {
    cap: Vec<u32>,
    leaves: Vec<Vec<(usize, u32)>>,
}

fn project(fq: &FeatureBag, lf: &LeafFeatures, n_leaves: usize) -> Projected {
    let mut dense: FxHashMap<u64, usize> = FxHashMap::default();
    dense.reserve(fq.set_len());
    let cap: Vec<u32> = fq
        .entries()
        .iter()
        .enumerate()
        .map(|(i, &(id, c))| {
            dense.insert(id, i);
            c
        })
        .collect();
    let leaves = (0..n_leaves)
        .map(|i| {
            lf.per_leaf
                .get(i)
                .map(|bag| {
                    bag.entries()
                        .iter()
                        .filter_map(|&(id, c)| dense.get(&id).map(|&d| (d, c)))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    Projected { cap, leaves }
}

#[inline]
fn gain(cap: &[u32], cur: &[u32], leaf: &[(usize, u32)]) -> u64 {
    leaf.iter()
        .map(|&(d, c)| {
            let q = cap[d];
            let f = cur[d];
            u64::from(q.min(f.saturating_add(c)) - q.min(f))
        })
        .sum()
}

#[inline]
fn add(cur: &mut [u32], leaf: &[(usize, u32)]) {
    for &(d, c) in leaf {
        cur[d] = cur[d].saturating_add(c);
    }
}

/// `Prune(F(q), m)` with lazy-greedy evaluation.
pub fn prune(fq: &FeatureBag, m: &SimplifiedTree, lf: &LeafFeatures) -> Pruned {
    let n = m.leaves.len();
    debug_assert_eq!(lf.per_leaf.len(), n, "LeafFeatures must align with tree.leaves");
    let p = project(fq, lf, n);
    let mut cur = vec![0u32; p.cap.len()];
    let mut keep = vec![false; n];
    let mut sim: u64 = 0;

    // Initial gains are exact for F = ∅; keys are unique thanks to the index.
    let mut heap: BinaryHeap<(u64, Reverse<usize>)> = (0..n)
        .filter_map(|i| {
            let g = gain(&p.cap, &cur, &p.leaves[i]);
            (g > 0).then_some((g, Reverse(i)))
        })
        .collect();

    while let Some((_, Reverse(i))) = heap.pop() {
        let g = gain(&p.cap, &cur, &p.leaves[i]);
        if g == 0 {
            // Gains never increase, so this leaf can never be picked again.
            continue;
        }
        let fresh = (g, Reverse(i));
        if heap.peek().is_none_or(|top| fresh >= *top) {
            // Every stored key upper-bounds its true key, so `fresh` is the exact argmax.
            keep[i] = true;
            sim += g;
            add(&mut cur, &p.leaves[i]);
        } else {
            heap.push(fresh);
        }
    }

    let tree = m.retain(&keep);
    Pruned { keep, sim, tree }
}

/// Reference `O(|N|²)` implementation of Algorithm 1 used to validate [`prune`].
#[cfg(test)]
pub(crate) fn prune_naive(fq: &FeatureBag, m: &SimplifiedTree, lf: &LeafFeatures) -> Pruned {
    let n = m.leaves.len();
    let p = project(fq, lf, n);
    let mut cur = vec![0u32; p.cap.len()];
    let mut keep = vec![false; n];
    let mut sim = 0u64;
    loop {
        let mut best: Option<(u64, usize)> = None;
        for i in (0..n).filter(|&i| !keep[i]) {
            let g = gain(&p.cap, &cur, &p.leaves[i]);
            if best.is_none_or(|(bg, _)| g > bg) {
                best = Some((g, i));
            }
        }
        match best {
            Some((g, i)) if g > 0 => {
                keep[i] = true;
                sim += g;
                add(&mut cur, &p.leaves[i]);
            }
            _ => break,
        }
    }
    let tree = m.retain(&keep);
    Pruned { keep, sim, tree }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use sillage_core::{Lang, Node, NodeKind, Span};
    use std::sync::Arc;

    /// Flat tree: root `"#…#"` with `n` token leaves (n >= 2) or a single-token tree.
    fn flat_tree(n: usize) -> SimplifiedTree {
        let source: Arc<str> = Arc::from("x ".repeat(n.max(1)));
        if n <= 1 {
            let mut t = SimplifiedTree::empty(Lang::Java, source);
            if n == 1 {
                t.nodes.push(Node {
                    kind: NodeKind::Token { text: "x".into(), var: None },
                    parent: None,
                    index_in_parent: 0,
                    children: vec![],
                    span: Span::new(0, 1),
                    dropped_children: false,
                });
                t.leaves.push(0);
            }
            return t;
        }
        let mut nodes = vec![Node {
            kind: NodeKind::Internal { label: "#".repeat(n).into() },
            parent: None,
            index_in_parent: 0,
            children: (1..=n as u32).collect(),
            span: Span::new(0, (2 * n - 1) as u32),
            dropped_children: false,
        }];
        for i in 0..n {
            nodes.push(Node {
                kind: NodeKind::Token { text: "x".into(), var: None },
                parent: Some(0),
                index_in_parent: (i + 1) as u16,
                children: vec![],
                span: Span::new((2 * i) as u32, (2 * i + 1) as u32),
                dropped_children: false,
            });
        }
        SimplifiedTree { lang: Lang::Java, source, nodes, leaves: (1..=n as u32).collect() }
    }

    fn bag_strategy() -> impl Strategy<Value = FeatureBag> {
        prop::collection::vec((0u64..12, 1u32..4), 0..10).prop_map(FeatureBag::from_weighted)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]
        #[test]
        fn lazy_matches_naive(
            q in bag_strategy(),
            leaves in prop::collection::vec(bag_strategy(), 0..24),
        ) {
            let tree = flat_tree(leaves.len());
            let lf = LeafFeatures { per_leaf: leaves };
            let a = prune(&q, &tree, &lf);
            let b = prune_naive(&q, &tree, &lf);
            prop_assert_eq!(&a.keep, &b.keep);
            prop_assert_eq!(a.sim, b.sim);
            prop_assert_eq!(a.tree, b.tree);
            prop_assert!(a.sim <= q.total());
        }
    }
}
