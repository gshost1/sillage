//! Phase II (Paper §3.3.2): prune every Phase I candidate against the query and rerank.

use rayon::prelude::*;
use sillage_core::{FeatureBag, Featurizer, MethodId, SimplifiedTree};

use crate::{Ranked, prune};

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
