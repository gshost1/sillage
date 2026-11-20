//! Phase III `Intersect(τ, q)` (Paper §3.3.3; DESIGN.md Decision 2).

use sillage_core::{FeatureBag, Featurizer, IntersectOp, Params, SimplifiedTree, Singleton};

use crate::{Ranked, prune};

pub fn intersect(
    tuple: &[usize],
    fq: &FeatureBag,
    n2: &[Ranked],
    params: &Params,
    featurizer: &dyn Featurizer,
) -> SimplifiedTree {
    match tuple {
        [] => panic!("intersect requires a non-empty tuple"),
        [i1] => {
            let tree = &n2[*i1].tree;
            match params.singleton {
                // §4.3.5: "no intersection ... the full method bodies without any pruning".
                Singleton::FullBody => tree.clone(),
                Singleton::PrunedToQuery => prune(fq, tree, &featurizer.featurize(tree)).tree,
            }
        }
        [i1, i2] => {
            let f2 = featurizer.featurize(&n2[*i2].tree).total();
            let combined = match params.intersect_union {
                IntersectOp::AsPaper | IntersectOp::AlwaysSum => f2.sum(fq),
                IntersectOp::AlwaysUnion => f2.union(fq),
            };
            let tree = &n2[*i1].tree;
            prune(&combined, tree, &featurizer.featurize(tree)).tree
        }
        [prefix @ .., ik] => {
            let prev = intersect(prefix, fq, n2, params, featurizer);
            let fk = featurizer.featurize(&n2[*ik].tree).total();
            let combined = match params.intersect_union {
                IntersectOp::AsPaper | IntersectOp::AlwaysUnion => fk.union(fq),
                IntersectOp::AlwaysSum => fk.sum(fq),
            };
            prune(&combined, &prev, &featurizer.featurize(&prev)).tree
        }
    }
}
