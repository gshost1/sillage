mod common;

use common::*;
use proptest::prelude::*;
use sillage_core::{FeatureBag, LeafFeatures, SimplifiedTree};
use sillage_engine::prune;

/// Literal Algorithm 1: argmax over remaining leaves of `SimScore(F(m1), F ⊎ F(n'))`,
/// earliest index on ties, stop when no strict improvement.
fn algorithm1_literal(fq: &FeatureBag, m: &SimplifiedTree, lf: &LeafFeatures) -> (Vec<bool>, u64) {
    let mut keep = vec![false; m.leaves.len()];
    let mut f = FeatureBag::new();
    loop {
        let base = fq.intersect_count(&f);
        let mut best: Option<(u64, usize)> = None;
        for i in (0..keep.len()).filter(|&i| !keep[i]) {
            let s = fq.intersect_count(&f.sum(&lf.per_leaf[i]));
            if best.is_none_or(|(bs, _)| s > bs) {
                best = Some((s, i));
            }
        }
        match best {
            Some((s, i)) if s > base => {
                keep[i] = true;
                f = f.sum(&lf.per_leaf[i]);
            }
            _ => return (keep, base),
        }
    }
}

fn bag() -> impl Strategy<Value = FeatureBag> {
    prop::collection::vec((0u64..10, 1u32..4), 0..8).prop_map(FeatureBag::from_weighted)
}

static NAMES: [&str; 32] = [
    "l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9", "l10", "l11", "l12", "l13", "l14",
    "l15", "l16", "l17", "l18", "l19", "l20", "l21", "l22", "l23", "l24", "l25", "l26", "l27",
    "l28", "l29", "l30", "l31",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Lazy-greedy `prune` picks exactly the leaves the literal greedy loop picks.
    #[test]
    fn lazy_prune_equals_literal_algorithm1(
        q in bag(),
        leaves in prop::collection::vec(bag(), 2..32),
    ) {
        let tree = flat(&NAMES[..leaves.len()]);
        let lf = LeafFeatures { per_leaf: leaves };
        let got = prune(&q, &tree, &lf);
        let (keep, sim) = algorithm1_literal(&q, &tree, &lf);
        prop_assert_eq!(&got.keep, &keep);
        prop_assert_eq!(got.sim, sim);
        prop_assert_eq!(got.tree, tree.retain(&keep));
    }
}

#[test]
fn pair_intersection_x1y2_y2z3() {
    let stmt = |a, b| n(vec![n(vec![t(a), k("="), t(b)]), k(";")]);
    let q = build(n(vec![stmt("x", "1"), stmt("y", "2")]));
    let m = build(n(vec![stmt("y", "2"), stmt("z", "3")]));
    let fz = TestFeaturizer;
    let p = prune(&fq(&q, &fz), &m, &fz.featurize(&m));
    assert_eq!(p.keep, vec![true, true, false, false]);
    assert_eq!(text(&p.tree), "y = 2 ;");
    assert_eq!(sexpr(&p.tree), sexpr(&build(stmt("y", "2"))));
    p.tree.validate().unwrap();
}

#[test]
fn swapped_nested_ifs_keep_both_ifs() {
    let cond = |a, op, b| n(vec![k("("), n(vec![t(a), k(op), t(b)]), k(")")]);
    let assign = |a, b| n(vec![n(vec![t(a), k("="), t(b)]), k(";")]);
    // x = 1; if (y > 1) if (z < 0) w = 4;
    let q = build(n(vec![
        assign("x", "1"),
        n(vec![k("if"), cond("y", ">", "1"), n(vec![k("if"), cond("z", "<", "0"), assign("w", "4")])]),
    ]));
    // if (z < 0) if (y > 1) w = 4; v = 10;
    let m = build(n(vec![
        n(vec![k("if"), cond("z", "<", "0"), n(vec![k("if"), cond("y", ">", "1"), assign("w", "4")])]),
        assign("v", "10"),
    ]));
    let fz = TestFeaturizer;
    let p = prune(&fq(&q, &fz), &m, &fz.featurize(&m));
    assert_eq!(leaf_texts(&p.tree), ["z", "0", "y", "1", "w", "4"]);
    assert_eq!(text(&p.tree), "if ( z < 0 ) if ( y > 1 ) w = 4 ;");
    p.tree.validate().unwrap();
}

#[test]
fn empty_query_keeps_nothing() {
    let m = flat(&["a", "b"]);
    let fz = TokenFeaturizer;
    let p = prune(&FeatureBag::new(), &m, &fz.featurize(&m));
    assert_eq!(p.keep, vec![false, false]);
    assert_eq!(p.sim, 0);
    assert!(p.tree.is_empty());
}

#[test]
fn multiset_capacity_is_respected() {
    // q = {a:2}; m has three `a` leaves: only the first two are kept.
    let fz = TokenFeaturizer;
    let q = fq(&flat(&["a", "a"]), &fz);
    let m = flat(&["a", "b", "a", "a"]);
    let p = prune(&q, &m, &fz.featurize(&m));
    assert_eq!(p.keep, vec![true, false, true, false]);
    assert_eq!(p.sim, 2);
}
