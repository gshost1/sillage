//! Phase II (Paper §3.3.2): prune every Phase I candidate against the query and rerank.
//!
//! Candidates are ranked by the key [`TieKey`]: pruned similarity `sim` first
//! (Aroma's ranking, unchanged), then structural tie-breakers that only decide
//! between candidates with equal `sim` (Decision 24).

use std::cmp::Ordering;

use rayon::prelude::*;
use sillage_core::{FeatureBag, Featurizer, MethodId, SimplifiedTree};
use sillage_index::Hit;

use crate::prune::prune_keep;
use crate::{Pruned, Ranked, prune};

/// The Phase II ranking key of one candidate (Decision 24), in priority order:
///
/// 1. `sim` DESC: the pruned similarity, Aroma's ranking;
/// 2. `overlap` DESC: the Phase I set overlap `|S(F(q)) ∩ S(F(m))|` (the
///    Stage 5 secondary key, unchanged);
/// 3. `full_sim` DESC: `|F_full(q) ∩ F(m)|`, the multiset overlap with the
///    query's *full* featurization, whose cross-line sibling and variable-usage
///    features `F(q)` drops (query mode). Among methods that contain the same
///    query-mode features, it prefers the one where the query's lines are laid
///    out as in the snippet;
/// 4. `size` ASC: `|F(m)|`. Among methods that match the query equally, it
///    prefers the tighter one, with fewer features outside the query;
/// 5. `id` ASC: a total order, so the ranking is deterministic.
///
/// Every key is computed from alpha-renamed features (`#VAR`), so the ranking
/// is invariant under renaming locals. Keys 2–5 only order candidates with
/// equal `sim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TieKey {
    pub sim: u64,
    pub overlap: u32,
    pub full_sim: u64,
    pub size: u64,
    pub id: MethodId,
}

impl TieKey {
    /// `Less` = ranks earlier.
    pub fn rank_cmp(&self, b: &TieKey) -> Ordering {
        b.sim
            .cmp(&self.sim)
            .then(b.overlap.cmp(&self.overlap))
            .then(b.full_sim.cmp(&self.full_sim))
            .then(self.size.cmp(&b.size))
            .then(self.id.cmp(&b.id))
    }

    /// The best key hit `h` could reach once loaded, given an upper bound on
    /// its `full_sim` (`sim ≤ h.bound`, `size ≥ h.size`).
    fn optimistic(h: &Hit, full_sim: u64) -> TieKey {
        TieKey { sim: h.bound, overlap: h.overlap, full_sim, size: h.size, id: h.id }
    }
}

/// No pre-load information on `full_sim`: every hit tied with the `k`-th
/// `(sim, overlap)` and not larger than it is loaded.
pub fn no_full_bound(_: MethodId) -> u64 {
    u64::MAX
}

/// `full_sim = |F_full(q) ∩ F(m)|` and `size = |F(m)|` for a featurized method,
/// counted over the per-leaf bags without building `F(m)`.
fn tie_fields(fq_full: &FeatureBag, lf: &sillage_core::LeafFeatures) -> (u64, u64) {
    let q = fq_full.entries();
    let mut have = vec![0u64; q.len()];
    let mut size = 0u64;
    for bag in &lf.per_leaf {
        for &(f, c) in bag.entries() {
            size += u64::from(c);
            if let Ok(i) = q.binary_search_by_key(&f, |e| e.0) {
                have[i] += u64::from(c);
            }
        }
    }
    let full_sim = q.iter().zip(&have).map(|(&(_, qc), &m)| m.min(u64::from(qc))).sum();
    (full_sim, size)
}

/// Exhaustive Phase II: prunes and ranks every candidate.
///
/// `fq` is the query-mode `F(q)` that prune matches; `fq_full` is the full
/// featurization of the query (cross-line sibling and variable-usage features
/// included), used only to break ties in `sim`.
pub fn rerank(
    fq: &FeatureBag,
    fq_full: &FeatureBag,
    cands: Vec<(MethodId, u32, SimplifiedTree)>,
    featurizer: &dyn Featurizer,
) -> Vec<Ranked> {
    let fq_total = fq.total();
    let mut out: Vec<Ranked> = cands
        .into_par_iter()
        .map(|(id, overlap, tree)| {
            let lf = featurizer.featurize(&tree);
            let (full_sim, size) = tie_fields(fq_full, &lf);
            let pruned = prune(fq, &tree, &lf);
            let score = if fq_total == 0 {
                0.0
            } else {
                pruned.sim as f64 / fq_total as f64
            };
            Ranked { id, overlap, sim: pruned.sim, full_sim, size, score, pruned, tree }
        })
        .collect();
    out.sort_by(|a, b| a.key().rank_cmp(&b.key()));
    out
}

/// Exact top-`k` of [`rerank`] over Phase I `hits`, parsing as few candidates as
/// possible. Returns the ranked list and the number of candidates loaded.
///
/// Each hit's [`Hit::bound`] upper-bounds its pruned similarity (prune only
/// covers query features the method has, at most `min(q_f, m_f)` times each).
/// Candidates are loaded (`load`, usually a re-parse) in decreasing-bound order,
/// in parallel chunks; once `k` results are known, a candidate whose best
/// possible key ([`TieKey`] with `sim = bound`) cannot beat the `k`-th result is
/// skipped, and the scan stops at the first bound below the `k`-th similarity.
/// When `(bound, overlap)` ties the `k`-th `(sim, overlap)`, the tie-break
/// fields decide: `full_bound(id)` (called only then) upper-bounds the hit's
/// `full_sim`, [`Hit::size`] lower-bounds its `size`, and the hit is loaded only
/// if that optimistic key beats the `k`-th ([`no_full_bound`] gives no
/// `full_sim` information). Hits are loaded in `(bound DESC, overlap DESC,
/// size ASC, id ASC)` order, so tight methods in a tied group come first. Only the final top `k` build pruned trees. The output equals
/// `rerank(…)` truncated to `k` whenever every bound is valid.
///
/// With `min_score = Some(τ)`, only candidates with `score > τ` are kept (and
/// those whose bound cannot exceed `τ` are never loaded): the output is then the
/// longest prefix of that ranking whose scores exceed `τ`, which is all Phase III
/// uses.
pub fn rerank_top<L, B>(
    fq: &FeatureBag,
    fq_full: &FeatureBag,
    mut hits: Vec<Hit>,
    k: usize,
    min_score: Option<f64>,
    featurizer: &dyn Featurizer,
    load: L,
    full_bound: B,
) -> (Vec<Ranked>, usize)
where
    L: Fn(MethodId) -> Option<SimplifiedTree> + Sync,
    B: Fn(MethodId) -> u64,
{
    let fq_total = fq.total();
    let full_cap = fq_full.total();
    let score_of = |sim: u64| if fq_total == 0 { 0.0 } else { sim as f64 / fq_total as f64 };
    let admits = |sim: u64| min_score.is_none_or(|t| score_of(sim) > t);
    hits.retain(|h| admits(h.bound));
    struct Scored {
        key: TieKey,
        keep: Vec<bool>,
        tree: SimplifiedTree,
    }
    if k == 0 || hits.is_empty() {
        return (Vec::new(), 0);
    }
    hits.sort_unstable_by(|a, b| {
        b.bound.cmp(&a.bound).then(b.overlap.cmp(&a.overlap)).then(a.size.cmp(&b.size)).then(a.id.cmp(&b.id))
    });

    let first_chunk = k.max(16);
    let next_chunk = (k / 4).max(16);
    let mut best: Vec<Scored> = Vec::with_capacity(k + first_chunk);
    let mut loaded = 0usize;
    let mut i = 0usize;
    while i < hits.len() {
        let kth = (best.len() >= k).then(|| best[k - 1].key);
        if let Some(t) = kth
            && hits[i].bound < t.sim
        {
            break; // bounds are sorted: nothing left can reach the top k
        }
        let end = (i + if i == 0 { first_chunk } else { next_chunk }).min(hits.len());
        let chunk: Vec<Hit> = hits[i..end]
            .iter()
            .filter(|h| {
                let Some(t) = kth else { return true };
                let beats = |full_sim| TieKey::optimistic(h, full_sim).rank_cmp(&t).is_lt();
                // `full_sim ≤ |F_full(q)|`; call `full_bound` only when the
                // coarse bounds cannot decide.
                beats(full_cap) && (t.sim < h.bound || t.overlap < h.overlap || beats(full_bound(h.id).min(full_cap)))
            })
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
                if !admits(sim) {
                    return None;
                }
                let (full_sim, size) = tie_fields(fq_full, &lf);
                let key = TieKey { sim, overlap: h.overlap, full_sim, size, id: h.id };
                Some(Scored { key, keep, tree })
            })
            .collect();
        best.extend(scored);
        best.sort_unstable_by(|a, b| a.key.rank_cmp(&b.key));
        best.truncate(k);
    }

    let ranked = best
        .into_par_iter()
        .map(|s| {
            let TieKey { sim, overlap, full_sim, size, id } = s.key;
            let pruned = Pruned { tree: s.tree.retain(&s.keep), keep: s.keep, sim };
            Ranked { id, overlap, sim, full_sim, size, score: score_of(sim), pruned, tree: s.tree }
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

    /// Hits for `trees` against `fq`, with each bound (`sim` over, `size`
    /// under) loosened by `slack`.
    fn hits_for(fq: &FeatureBag, trees: &[SimplifiedTree], slack: &[(u64, u64)]) -> Vec<Hit> {
        trees
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let total = NumFeaturizer.featurize(t).total();
                let (bound_slack, size_slack) = slack.get(i).copied().unwrap_or((0, 0));
                Hit {
                    id: MethodId(i as u32),
                    overlap: fq.set_intersect_count(&total),
                    bound: fq.intersect_count(&total) + bound_slack,
                    size: total.total().saturating_sub(size_slack),
                }
            })
            .collect()
    }

    fn assert_same(top: &[Ranked], full: &[Ranked]) -> Result<(), TestCaseError> {
        prop_assert_eq!(top.len(), full.len());
        for (a, b) in top.iter().zip(full) {
            prop_assert_eq!(a.id, b.id);
            prop_assert_eq!(a.sim, b.sim);
            prop_assert_eq!(a.overlap, b.overlap);
            prop_assert_eq!(a.full_sim, b.full_sim);
            prop_assert_eq!(a.size, b.size);
            prop_assert_eq!(&a.pruned.keep, &b.pruned.keep);
            prop_assert_eq!(&a.pruned.tree, &b.pruned.tree);
            prop_assert_eq!(&a.tree, &b.tree);
        }
        Ok(())
    }

    /// Sim ties are broken by `full_sim` DESC, then `size` ASC; never by id
    /// while those differ.
    #[test]
    fn ties_prefer_full_query_match_then_tighter_method() {
        let fq = FeatureBag::from_unsorted([1, 2]);
        let full = FeatureBag::from_unsorted([1, 2, 9]);
        let trees = vec![
            flat_tree(&[1, 2, 5, 5, 5]), // sim 2, full 2, size 5
            flat_tree(&[1, 2, 5]),       // sim 2, full 2, size 3
            flat_tree(&[1, 2, 9, 5, 5, 5, 5]), // sim 2, full 3, size 7
            flat_tree(&[1]),             // sim 1
        ];
        let cands = trees.iter().enumerate().map(|(i, t)| (MethodId(i as u32), 1, t.clone())).collect();
        let ranked = rerank(&fq, &full, cands, &NumFeaturizer);
        assert_eq!(ranked.iter().map(|r| r.id.0).collect::<Vec<_>>(), [2, 1, 0, 3]);
        assert_eq!((ranked[0].full_sim, ranked[0].size), (3, 7));
        let hits = hits_for(&fq, &trees, &[]);
        for k in 1..=4 {
            for exact in [true, false] {
                let tb = |id| if exact { exact_full_bound(&full, &trees, id) } else { no_full_bound(id) };
                let (top, _) = rerank_top(&fq, &full, hits.clone(), k, None, &NumFeaturizer, |id| Some(trees[id.0 as usize].clone()), tb);
                assert_eq!(top.iter().map(|r| r.id.0).collect::<Vec<_>>(), [2, 1, 0, 3][..k]);
            }
        }
    }

    /// Exact `full_sim` from the trees themselves.
    fn exact_full_bound(full: &FeatureBag, trees: &[SimplifiedTree], id: MethodId) -> u64 {
        full.intersect_count(&NumFeaturizer.featurize(&trees[id.0 as usize]).total())
    }

    /// A hit whose `(bound, overlap)` equals the k-th `(sim, overlap)` must
    /// still be considered: it can win the tie-break (here the last hit in
    /// bound order wins rank 1). Without tie bounds every tied hit is loaded;
    /// with exact ones only the hits that can still win are.
    #[test]
    fn rerank_top_loads_hits_tied_at_the_kth_similarity() {
        let fq = FeatureBag::from_unsorted([1, 2]);
        let full = FeatureBag::from_unsorted([1, 2, 9]);
        // Method 40 is the largest, so it is loaded last among the size-sorted ties.
        let mut trees: Vec<SimplifiedTree> = (0..40).map(|_| flat_tree(&[1, 2, 7])).collect();
        trees.push(flat_tree(&[1, 2, 9, 7, 7]));
        let hits = hits_for(&fq, &trees, &[]);
        let load = |id: MethodId| Some(trees[id.0 as usize].clone());
        let (top, loaded) = rerank_top(&fq, &full, hits.clone(), 1, None, &NumFeaturizer, load, no_full_bound);
        assert_eq!(top[0].id, MethodId(40));
        assert_eq!(loaded, 41);
        let (top, loaded) = rerank_top(&fq, &full, hits, 1, None, &NumFeaturizer, load, |id| exact_full_bound(&full, &trees, id));
        assert_eq!(top[0].id, MethodId(40));
        assert!(loaded < 41, "exact tie bounds should skip losing ties, loaded {loaded}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        /// `tie_fields` equals the multiset definitions on `F(m)`.
        #[test]
        fn tie_fields_match_definitions(
            q in prop::collection::vec(0u64..12, 0..12),
            m in prop::collection::vec(0u64..12, 1..20),
        ) {
            let full = FeatureBag::from_unsorted(q);
            let lf = NumFeaturizer.featurize(&flat_tree(&m));
            let fm = lf.total();
            prop_assert_eq!(tie_fields(&full, &lf), (full.intersect_count(&fm), fm.total()));
        }

        /// The bounded top-k equals the exhaustive rerank truncated to k, for exact
        /// and for loose (but valid) bounds, including heavy sim ties broken by a
        /// full query featurization that differs from `fq`.
        #[test]
        fn rerank_top_matches_exhaustive(
            q in prop::collection::vec(0u64..8, 1..12),
            q_extra in prop::collection::vec(0u64..12, 0..6),
            methods in prop::collection::vec(prop::collection::vec(0u64..12, 1..10), 0..60),
            slack in prop::collection::vec((0u64..3, 0u64..3), 60),
            full_slack in prop::collection::vec(0u64..3, 60),
            bounds in 0u8..3,
            k in 0usize..20,
            unparsable in 0usize..60,
            tau in prop::option::of(0.0f64..1.0),
        ) {
            let fq = FeatureBag::from_unsorted(q.clone());
            let full = FeatureBag::from_unsorted(q.into_iter().chain(q_extra));
            let trees: Vec<SimplifiedTree> = methods.iter().map(|m| flat_tree(m)).collect();
            let hits = hits_for(&fq, &trees, &slack);
            let load = |id: MethodId| (id.0 as usize != unparsable).then(|| trees[id.0 as usize].clone());
            let cands: Vec<_> = hits.iter().filter_map(|h| Some((h.id, h.overlap, load(h.id)?))).collect();
            let mut exhaustive = rerank(&fq, &full, cands, &NumFeaturizer);
            for w in exhaustive.windows(2) {
                prop_assert!(w[0].key().rank_cmp(&w[1].key()).is_lt());
                prop_assert!(w[0].sim >= w[1].sim);
            }
            exhaustive.truncate(k);
            if let Some(t) = tau {
                exhaustive.retain(|r| r.score > t);
            }
            // full_sim bounds: none, exact, or loose but valid.
            let full_bound = |id: MethodId| match bounds {
                0 => no_full_bound(id),
                1 => exact_full_bound(&full, &trees, id),
                _ => exact_full_bound(&full, &trees, id) + full_slack[id.0 as usize],
            };
            let (top, loaded) = rerank_top(&fq, &full, hits.clone(), k, tau, &NumFeaturizer, load, full_bound);
            prop_assert!(loaded <= hits.len());
            assert_same(&top, &exhaustive)?;
        }

        /// The tie-break never changes the primary ranking: the multiset of `sim`
        /// values at each rank equals the one of a sim-only sort.
        #[test]
        fn tie_break_preserves_sim_order(
            q in prop::collection::vec(0u64..8, 1..12),
            q_extra in prop::collection::vec(0u64..12, 0..6),
            methods in prop::collection::vec(prop::collection::vec(0u64..12, 1..10), 0..40),
        ) {
            let fq = FeatureBag::from_unsorted(q.clone());
            let full = FeatureBag::from_unsorted(q.into_iter().chain(q_extra));
            let cands: Vec<_> = methods.iter().enumerate().map(|(i, m)| (MethodId(i as u32), 0, flat_tree(m))).collect();
            let a: Vec<u64> = rerank(&fq, &full, cands.clone(), &NumFeaturizer).iter().map(|r| r.sim).collect();
            let mut b: Vec<u64> = rerank(&fq, &fq, cands, &NumFeaturizer).iter().map(|r| r.sim).collect();
            b.sort_unstable_by(|x, y| y.cmp(x));
            prop_assert_eq!(a, b);
        }
    }
}
