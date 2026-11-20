//! Phase II (Paper §3.3.2): prune every Phase I candidate against the query and rerank.

use rayon::prelude::*;
use sillage_core::{FeatureBag, Featurizer, MethodId, SimplifiedTree};
use sillage_index::Hit;

use crate::prune::prune_keep;
use crate::{Pruned, Ranked, prune};

pub fn rerank(
    fq: &FeatureBag,
    cands: Vec<(MethodId, u32, SimplifiedTree)>,
    featurizer: &dyn Featurizer,
) -> Vec<Ranked> {
    let fq_total = fq.total();
    let mut out: Vec<Ranked> = cands
        .into_par_iter()
        .map(|(id, overlap, tree)| {
            let lf = featurizer.featurize(&tree);
            let pruned = prune(fq, &tree, &lf);
            let score = if fq_total == 0 {
                0.0
            } else {
                pruned.sim as f64 / fq_total as f64
            };
            Ranked { id, overlap, sim: pruned.sim, score, pruned, tree }
        })
        .collect();
    out.sort_by(|a, b| {
        b.sim
            .cmp(&a.sim)
            .then_with(|| b.overlap.cmp(&a.overlap))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Exact top-`k` of [`rerank`] over Phase I `hits`, parsing as few candidates as
/// possible. Returns the ranked list and the number of candidates loaded.
///
/// Each hit's [`Hit::bound`] upper-bounds its pruned similarity (prune only
/// covers query features the method has, at most `min(q_f, m_f)` times each).
/// Candidates are loaded (`load`, usually a re-parse) in decreasing-bound order,
/// in parallel chunks; once `k` results are known, a candidate whose best possible
/// key `(bound, overlap, id)` cannot beat the `k`-th result is skipped, and the
/// scan stops at the first bound below the `k`-th similarity. Only the final top
/// `k` build pruned trees. The output equals `rerank(…)` truncated to `k`
/// whenever every bound is valid.
///
/// With `min_score = Some(τ)`, only candidates with `score > τ` are kept (and
/// those whose bound cannot exceed `τ` are never loaded): the output is then the
/// longest prefix of that ranking whose scores exceed `τ`, which is all Phase III
/// uses.
pub fn rerank_top<L>(
    fq: &FeatureBag,
    mut hits: Vec<Hit>,
    k: usize,
    min_score: Option<f64>,
    featurizer: &dyn Featurizer,
    load: L,
) -> (Vec<Ranked>, usize)
where
    L: Fn(MethodId) -> Option<SimplifiedTree> + Sync,
{
    let fq_total = fq.total();
    let score_of = |sim: u64| if fq_total == 0 { 0.0 } else { sim as f64 / fq_total as f64 };
    let admits = |sim: u64| min_score.is_none_or(|t| score_of(sim) > t);
    hits.retain(|h| admits(h.bound));
    struct Scored {
        id: MethodId,
        overlap: u32,
        sim: u64,
        keep: Vec<bool>,
        tree: SimplifiedTree,
    }
    // `Less` = ranks earlier (sim DESC, overlap DESC, id ASC), as in `rerank`.
    let order = |a: (u64, u32, MethodId), b: (u64, u32, MethodId)| {
        b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2))
    };
    if k == 0 || hits.is_empty() {
        return (Vec::new(), 0);
    }
    hits.sort_unstable_by(|a, b| order((a.bound, a.overlap, a.id), (b.bound, b.overlap, b.id)));

    let first_chunk = k.max(16);
    let next_chunk = (k / 4).max(16);
    let mut best: Vec<Scored> = Vec::with_capacity(k + first_chunk);
    let mut loaded = 0usize;
    let mut i = 0usize;
    while i < hits.len() {
        let kth = (best.len() >= k).then(|| {
            let b = &best[k - 1];
            (b.sim, b.overlap, b.id)
        });
        if let Some(t) = kth
            && hits[i].bound < t.0
        {
            break; // bounds are sorted: nothing left can reach the top k
        }
        let end = (i + if i == 0 { first_chunk } else { next_chunk }).min(hits.len());
        let chunk: Vec<Hit> = hits[i..end]
            .iter()
            .filter(|h| kth.is_none_or(|t| order((h.bound, h.overlap, h.id), t).is_lt()))
            .copied()
            .collect();
        i = end;
        loaded += chunk.len();
        let scored: Vec<Scored> = chunk
            .into_par_iter()
            .filter_map(|h| {
                let tree = load(h.id)?;
                let lf = featurizer.featurize(&tree);
                let (keep, sim) = prune_keep(fq, &tree, &lf);
                admits(sim).then_some(Scored { id: h.id, overlap: h.overlap, sim, keep, tree })
            })
            .collect();
        best.extend(scored);
        best.sort_unstable_by(|a, b| order((a.sim, a.overlap, a.id), (b.sim, b.overlap, b.id)));
        best.truncate(k);
    }

    let ranked = best
        .into_par_iter()
        .map(|s| {
            let score = score_of(s.sim);
            let pruned = Pruned { tree: s.tree.retain(&s.keep), keep: s.keep, sim: s.sim };
            Ranked { id: s.id, overlap: s.overlap, sim: s.sim, score, pruned, tree: s.tree }
        })
        .collect();
    (ranked, loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use sillage_core::{Lang, LeafFeatures, Node, NodeKind, Span};
    use std::sync::Arc;

    /// Feature ID of each leaf = its text parsed as a number.
    struct NumFeaturizer;

    impl Featurizer for NumFeaturizer {
        fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures {
            LeafFeatures {
                per_leaf: tree
                    .leaves
                    .iter()
                    .map(|&l| FeatureBag::from_unsorted([tree.label(l).parse::<u64>().unwrap()]))
                    .collect(),
            }
        }
    }

    fn flat_tree(ids: &[u64]) -> SimplifiedTree {
        let source: Arc<str> = Arc::from(" ".repeat(ids.len()));
        let mut nodes = vec![Node {
            kind: NodeKind::Internal { label: "#".repeat(ids.len()).into() },
            parent: None,
            index_in_parent: 0,
            children: (1..=ids.len() as u32).collect(),
            span: Span::new(0, ids.len() as u32),
            dropped_children: false,
        }];
        for (i, id) in ids.iter().enumerate() {
            nodes.push(Node {
                kind: NodeKind::Token { text: id.to_string().into(), var: None },
                parent: Some(0),
                index_in_parent: i as u16 + 1,
                children: vec![],
                span: Span::new(i as u32, i as u32 + 1),
                dropped_children: false,
            });
        }
        SimplifiedTree { lang: Lang::Java, source, nodes, leaves: (1..=ids.len() as u32).collect() }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        /// The bounded top-k equals the exhaustive rerank truncated to k, for exact
        /// and for loose (but valid) bounds, including heavy ties.
        #[test]
        fn rerank_top_matches_exhaustive(
            q in prop::collection::vec(0u64..8, 1..12),
            methods in prop::collection::vec(prop::collection::vec(0u64..8, 1..10), 0..60),
            slack in prop::collection::vec(0u64..3, 60),
            k in 0usize..20,
            unparsable in 0usize..60,
            tau in prop::option::of(0.0f64..1.0),
        ) {
            let fq = FeatureBag::from_unsorted(q);
            let trees: Vec<SimplifiedTree> = methods.iter().map(|m| flat_tree(m)).collect();
            let hits: Vec<Hit> = trees
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let total = NumFeaturizer.featurize(t).total();
                    Hit {
                        id: MethodId(i as u32),
                        overlap: fq.set_intersect_count(&total),
                        bound: fq.intersect_count(&total) + slack[i],
                    }
                })
                .collect();
            let load = |id: MethodId| (id.0 as usize != unparsable).then(|| trees[id.0 as usize].clone());
            let cands: Vec<_> = hits.iter().filter_map(|h| Some((h.id, h.overlap, load(h.id)?))).collect();
            let mut full = rerank(&fq, cands, &NumFeaturizer);
            full.truncate(k);
            if let Some(t) = tau {
                full.retain(|r| r.score > t);
            }
            let (top, loaded) = rerank_top(&fq, hits.clone(), k, tau, &NumFeaturizer, load);
            prop_assert!(loaded <= hits.len());
            prop_assert_eq!(top.len(), full.len());
            for (a, b) in top.iter().zip(&full) {
                prop_assert_eq!(a.id, b.id);
                prop_assert_eq!(a.sim, b.sim);
                prop_assert_eq!(a.overlap, b.overlap);
                prop_assert_eq!(&a.pruned.keep, &b.pruned.keep);
                prop_assert_eq!(&a.pruned.tree, &b.pruned.tree);
                prop_assert_eq!(&a.tree, &b.tree);
            }
        }
    }
}
