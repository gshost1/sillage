//! Phase III clustering (Paper §3.3.3; DESIGN.md Decisions 1, 3, 4, 6, 7).

use rayon::prelude::*;
use rustc_hash::FxHashSet;
use sillage_core::{ClusterTies, Dedup, FeatureBag, Featurizer, MAX_TUPLES, Params, SDenom};

use crate::Ranked;

const EPS: f64 = 1e-12;

/// A valid, deduplicated cluster: indices into `n2` plus its `l(τ)` and `s(τ)` scores.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCluster {
    pub tuple: Vec<usize>,
    pub l: f64,
    pub s: f64,
}

struct Tuple {
    /// Positions into the eligible list (strictly increasing).
    pos: Vec<usize>,
    cs_bag: FeatureBag,
    csq_bag: FeatureBag,
    l: f64,
    s: f64,
}

/// `(l, s)` for the given counts, or `None` when `csq == 0`.
fn ratios(cs: u64, csq: u64, denom: u64) -> Option<(f64, f64)> {
    if csq == 0 {
        return None;
    }
    let l = cs as f64 / csq as f64;
    let s = if denom == 0 { 0.0 } else { csq as f64 / denom as f64 };
    Some((l, s))
}

fn jaccard(a: &[usize], b: &[usize]) -> f64 {
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - inter;
    if union == 0 { 0.0 } else { inter as f64 / union as f64 }
}

/// Cheap necessary condition for `jaccard(a, b) > threshold` on strictly
/// increasing tuples: Jaccard is at most `min(|a|, |b|) / max(|a|, |b|)`, and is
/// 0 when the value ranges `[a_1, a_k]` and `[b_1, b_m]` are disjoint.
fn may_exceed(a: &[usize], b: &[usize], threshold: f64) -> bool {
    let (Some((&a0, &an)), Some((&b0, &bn))) = (a.first().zip(a.last()), b.first().zip(b.last())) else {
        return false;
    };
    let (lo, hi) = if a.len() <= b.len() { (a.len(), b.len()) } else { (b.len(), a.len()) };
    lo as f64 / hi as f64 > threshold && an >= b0 && bn >= a0
}

/// Clusters `N3` with their `l(τ)`/`s(τ)` scores, sorted and deduplicated.
pub fn cluster_scored(
    _fq: &FeatureBag,
    n2: &[Ranked],
    params: &Params,
    featurizer: &dyn Featurizer,
) -> Vec<ScoredCluster> {
    // Top η2 candidates above τ1, keeping their original `n2` indices.
    let eligible: Vec<usize> = n2
        .iter()
        .enumerate()
        .filter(|(_, r)| r.score > params.tau1)
        .map(|(i, _)| i)
        .take(params.eta2)
        .collect();
    if eligible.is_empty() {
        return Vec::new();
    }

    // Decision 7: F(Prune(F(q), m)) re-featurizes the pruned tree.
    let (full_f, pruned_f): (Vec<FeatureBag>, Vec<FeatureBag>) = eligible
        .par_iter()
        .map(|&i| {
            (
                featurizer.featurize(&n2[i].tree).total(),
                featurizer.featurize(&n2[i].pruned.tree).total(),
            )
        })
        .unzip();
    let (full_f, pruned_f) = (&full_f, &pruned_f);
    let denom_of = |p: usize| match params.s_denominator {
        SDenom::PrunedFirst => pruned_f[p].total(),
        SDenom::PaperLiteral => full_f[p].total(),
    };
    let valid = |l: f64, s: f64| l > params.tau2 && s > params.tau3;

    // C1.
    let mut frontier: Vec<Tuple> = (0..eligible.len())
        .filter_map(|p| {
            let (l, s) = ratios(full_f[p].total(), pruned_f[p].total(), denom_of(p))?;
            valid(l, s).then(|| Tuple {
                pos: vec![p],
                cs_bag: full_f[p].clone(),
                csq_bag: pruned_f[p].clone(),
                l,
                s,
            })
        })
        .collect();
    let mut seen: FxHashSet<Vec<usize>> = frontier.iter().map(|t| t.pos.clone()).collect();
    let mut all: Vec<(Vec<usize>, f64, f64)> =
        frontier.iter().map(|t| (t.pos.clone(), t.l, t.s)).collect();

    // C_{ℓ+1} = C_ℓ ∪ extensions; iterate until no new tuple appears. Each level
    // is grown in chunks of the frontier so that `all` never exceeds MAX_TUPLES,
    // even when one level of `ClusterTies::All` would produce far more tuples.
    let ties = match params.cluster_ties {
        ClusterTies::First => 1,
        ClusterTies::All => usize::MAX,
    };
    let extend = |t: &Tuple| -> Vec<Tuple> {
        let last = *t.pos.last().expect("non-empty tuple");
        let denom = denom_of(t.pos[0]);
        // Decision 3: the max ranges over every eligible j > i_k, valid or not.
        let scored: Vec<(usize, f64)> = (last + 1..eligible.len())
            .filter_map(|j| {
                let cs = t.cs_bag.intersect_count(&full_f[j]);
                let csq = t.csq_bag.intersect_count(&pruned_f[j]);
                ratios(cs, csq, denom).map(|(l, _)| (j, l))
            })
            .collect();
        let max_l = scored.iter().map(|&(_, l)| l).fold(f64::NEG_INFINITY, f64::max);
        scored
            .into_iter()
            .filter(|&(_, l)| (l - max_l).abs() <= EPS)
            .take(ties)
            .filter_map(|(j, _)| {
                let cs_bag = t.cs_bag.intersect(&full_f[j]);
                let csq_bag = t.csq_bag.intersect(&pruned_f[j]);
                let (l, s) = ratios(cs_bag.total(), csq_bag.total(), denom)?;
                valid(l, s).then(|| {
                    let mut pos = t.pos.clone();
                    pos.push(j);
                    Tuple { pos, cs_bag, csq_bag, l, s }
                })
            })
            .collect()
    };
    const CHUNK: usize = 64;
    'grow: while !frontier.is_empty() && all.len() < MAX_TUPLES {
        let mut next: Vec<Tuple> = Vec::new();
        for chunk in frontier.chunks(CHUNK) {
            let extensions: Vec<Tuple> = chunk.par_iter().flat_map_iter(extend).collect();
            for t in extensions {
                if all.len() >= MAX_TUPLES {
                    break 'grow;
                }
                if seen.insert(t.pos.clone()) {
                    all.push((t.pos.clone(), t.l, t.s));
                    next.push(t);
                }
            }
        }
        frontier = next;
    }
    all.truncate(MAX_TUPLES);

    // Map to n2 indices, then sort by (first ASC, len DESC, tuple ASC).
    let mut tuples: Vec<ScoredCluster> = all
        .into_iter()
        .map(|(pos, l, s)| ScoredCluster {
            tuple: pos.into_iter().map(|p| eligible[p]).collect(),
            l,
            s,
        })
        .collect();
    tuples.sort_by(|a, b| {
        a.tuple[0]
            .cmp(&b.tuple[0])
            .then_with(|| b.tuple.len().cmp(&a.tuple.len()))
            .then_with(|| a.tuple.cmp(&b.tuple))
    });

    // Decision 4: Jaccard dedup against all earlier tuples or only kept ones.
    let too_similar = |a: &[usize], b: &[usize]| may_exceed(a, b, params.jaccard) && jaccard(a, b) > params.jaccard;
    let mut kept: Vec<ScoredCluster> = Vec::new();
    for (idx, t) in tuples.iter().enumerate() {
        let dup = match params.dedup_against {
            Dedup::AllEarlier => tuples[..idx].iter().any(|e| too_similar(&t.tuple, &e.tuple)),
            Dedup::KeptOnly => kept.iter().any(|e| too_similar(&t.tuple, &e.tuple)),
        };
        if !dup {
            kept.push(t.clone());
        }
    }
    kept
}

pub fn cluster(
    fq: &FeatureBag,
    n2: &[Ranked],
    params: &Params,
    featurizer: &dyn Featurizer,
) -> Vec<Vec<usize>> {
    cluster_scored(fq, n2, params, featurizer)
        .into_iter()
        .map(|c| c.tuple)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{jaccard, may_exceed};

    #[test]
    fn may_exceed_is_a_sound_prefilter() {
        let tuples: Vec<Vec<usize>> = vec![vec![0], vec![1], vec![0, 1], vec![0, 2], vec![0, 1, 2], vec![3, 4], vec![1, 2, 3, 4]];
        for thr in [0.0, 0.3, 0.5, 0.9] {
            for a in &tuples {
                for b in &tuples {
                    if jaccard(a, b) > thr {
                        assert!(may_exceed(a, b, thr), "{a:?} {b:?} {thr}");
                    }
                }
            }
        }
        assert!(!may_exceed(&[0, 1], &[2, 3], 0.0));
        assert!(!may_exceed(&[0], &[0, 1, 2], 0.5));
    }

    #[test]
    fn jaccard_basics() {
        assert_eq!(jaccard(&[0, 1], &[0]), 0.5);
        assert_eq!(jaccard(&[0, 1, 2], &[0, 1]), 2.0 / 3.0);
        assert_eq!(jaccard(&[1], &[2]), 0.0);
    }
}
