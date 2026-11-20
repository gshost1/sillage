//! `SDenom`, `IntersectOp`, `Dedup` toggles and cluster-growth semantics (Decisions 1–4).
mod common;

use common::*;
use sillage_core::{ClusterTies, Dedup, IntersectOp, MethodId, Params, SDenom, Singleton};
use sillage_engine::{Ranked, cluster, cluster_scored, intersect, rerank};

fn rerank_flat(q: &[&'static str], trees: &[&[&'static str]]) -> (sillage_core::FeatureBag, Vec<Ranked>) {
    let fz = TokenFeaturizer;
    let fq = fq(&flat(q), &fz);
    let cands = trees
        .iter()
        .enumerate()
        .map(|(i, toks)| (MethodId(i as u32), 100 - i as u32, flat(toks)))
        .collect();
    let n2 = rerank(&fq, cands, &fz);
    (fq, n2)
}

/// Chain where (0) → (0,1) → (0,1,2) all grow validly.
fn chain() -> (sillage_core::FeatureBag, Vec<Ranked>) {
    rerank_flat(&["a"], &[&["a", "x", "y", "z"], &["a", "x", "y", "w"], &["a", "x", "v", "u"]])
}

#[test]
fn growth_reaches_fixed_point() {
    let (fq, n2) = chain();
    let params = Params { jaccard: 1.0, ..Params::default() }; // no dedup
    let got = cluster(&fq, &n2, &params, &TokenFeaturizer);
    let want: Vec<Vec<usize>> =
        vec![vec![0, 1, 2], vec![0, 1], vec![0], vec![1, 2], vec![1], vec![2]];
    assert_eq!(got, want);
    let scored = cluster_scored(&fq, &n2, &params, &TokenFeaturizer);
    assert_eq!(scored[0].l, 2.0); // cs = |{a, x}|, csq = |{a}|
    assert_eq!(scored[1].l, 3.0);
    assert!(scored.iter().all(|c| c.s == 1.0));
}

#[test]
fn dedup_all_earlier_vs_kept_only() {
    let (fq, n2) = chain();
    let fz = TokenFeaturizer;
    let all = Params { jaccard: 0.4, dedup_against: Dedup::AllEarlier, ..Params::default() };
    assert_eq!(cluster(&fq, &n2, &all, &fz), vec![vec![0, 1, 2]]);
    let kept = Params { dedup_against: Dedup::KeptOnly, ..all };
    assert_eq!(
        cluster(&fq, &n2, &kept, &fz),
        vec![vec![0, 1, 2], vec![0], vec![1], vec![2]]
    );
    // Paper default (0.5, strict >): (0) vs (0,1) has Jaccard exactly 0.5 and survives.
    let dflt = Params::default();
    assert_eq!(cluster(&fq, &n2, &dflt, &fz), vec![vec![0, 1, 2], vec![0], vec![1], vec![2]]);
}

#[test]
fn s_denominator_toggle() {
    let (fq, n2) = chain();
    let fz = TokenFeaturizer;
    let lit = Params { s_denominator: SDenom::PaperLiteral, ..Params::default() };
    assert!(cluster(&fq, &n2, &lit, &fz).is_empty()); // s = 1/4
    let lit_relaxed = Params { tau3: 0.2, ..lit };
    assert!(!cluster(&fq, &n2, &lit_relaxed, &fz).is_empty());
    let scored = cluster_scored(&fq, &n2, &lit_relaxed, &fz);
    assert!(scored.iter().all(|c| c.s == 0.25));
}

#[test]
fn ties_add_every_maximiser() {
    let (fq, n2) = rerank_flat(&["a"], &[&["a", "x", "y"], &["a", "x", "y", "p"], &["a", "x", "y", "q"]]);
    let params = Params { jaccard: 1.0, cluster_ties: ClusterTies::All, ..Params::default() };
    let got = cluster(&fq, &n2, &params, &TokenFeaturizer);
    assert!(got.contains(&vec![0, 1]) && got.contains(&vec![0, 2]), "{got:?}");
    // Default: only the earliest maximiser extends a tuple.
    let first = Params { jaccard: 1.0, ..Params::default() };
    let got = cluster(&fq, &n2, &first, &TokenFeaturizer);
    assert!(got.contains(&vec![0, 1]) && !got.contains(&vec![0, 2]), "{got:?}");
}

#[test]
fn many_identical_candidates_do_not_explode() {
    // 60 identical candidates: the literal rule would enumerate ~2^60 tuples.
    let docs: Vec<&[&str]> = (0..60).map(|_| &["a", "x", "y", "z"][..]).collect();
    let (fq, n2) = rerank_flat(&["a"], &docs);
    let t = std::time::Instant::now();
    let got = cluster(&fq, &n2, &Params::default(), &TokenFeaturizer);
    assert!(!got.is_empty());
    assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    let capped = Params { cluster_ties: ClusterTies::All, ..Params::default() };
    let t = std::time::Instant::now();
    let _ = cluster(&fq, &n2, &capped, &TokenFeaturizer);
    assert!(t.elapsed().as_secs() < 60, "{:?}", t.elapsed());
    // Stage 5: the cap holds *within* a level too. Without dedup, level 3 alone
    // has C(60, 3) = 34,220 tuples; growth must stop at exactly MAX_TUPLES.
    let no_dedup = Params { jaccard: 1.0, ..capped };
    let all = cluster_scored(&fq, &n2, &no_dedup, &TokenFeaturizer);
    assert_eq!(all.len(), sillage_core::MAX_TUPLES);
}

#[test]
fn max_l_ranges_over_invalid_candidates_too() {
    // Decision 3: from (T0), the max-l extension is T1 (invalid: s = 1/2), so no
    // extension happens even though T2 alone would be a valid extension.
    let fz = TokenFeaturizer;
    let params = Params { tau1: 0.4, jaccard: 1.0, ..Params::default() };
    let (fq, n2) = rerank_flat(
        &["a", "b"],
        &[&["a", "b", "x", "y", "z"], &["a", "x", "y", "z", "w"], &["a", "b", "x", "y", "k"]],
    );
    // Rerank order: T0 (sim 2), T2 (sim 2), T1 (sim 1).
    assert_eq!(n2.iter().map(|r| r.id.0).collect::<Vec<_>>(), [0, 2, 1]);
    let got = cluster(&fq, &n2, &params, &fz);
    assert!(got.iter().all(|t| t.len() == 1), "{got:?}");

    // Without T1, (T0, T2) is the max and valid.
    let (fq2, n2b) = rerank_flat(&["a", "b"], &[&["a", "b", "x", "y", "z"], &["a", "b", "x", "y", "k"]]);
    assert!(cluster(&fq2, &n2b, &params, &fz).contains(&vec![0, 1]));
}

#[test]
fn eta2_and_tau1_limit_candidates() {
    let (fq, n2) = chain();
    let fz = TokenFeaturizer;
    let params = Params { eta2: 1, ..Params::default() };
    assert_eq!(cluster(&fq, &n2, &params, &fz), vec![vec![0]]);
    let params = Params { tau1: 1.0, ..Params::default() };
    assert!(cluster(&fq, &n2, &params, &fz).is_empty());
}

#[test]
fn intersect_operator_toggle() {
    let fz = TokenFeaturizer;
    let q = fq(&flat(&["a"]), &fz);
    let n2 = vec![
        ranked(0, flat(&["a", "a", "a", "b"]), &q, &fz),
        ranked(1, flat(&["a", "a", "b"]), &q, &fz),
        ranked(2, flat(&["a", "a"]), &q, &fz),
    ];
    let run = |op, tuple: &[usize]| {
        let params = Params { intersect_union: op, ..Params::default() };
        leaf_texts(&intersect(tuple, &q, &n2, &params, &fz))
    };
    // Single (§4.3.5 default): the full method body, unpruned.
    assert_eq!(run(IntersectOp::AsPaper, &[0]), ["a", "a", "a", "b"]);
    // Single, literal §3.3.3 formula: Prune(F(q), N2(i1)).
    let literal = Params { singleton: Singleton::PrunedToQuery, ..Params::default() };
    assert_eq!(leaf_texts(&intersect(&[0], &q, &n2, &literal, &fz)), ["a"]);
    // Pair: F(N2(i2)) ⊎ F(q) = {a:3, b:1} vs F(N2(i2)) ∪ F(q) = {a:2, b:1}.
    assert_eq!(run(IntersectOp::AsPaper, &[0, 1]), ["a", "a", "a", "b"]);
    assert_eq!(run(IntersectOp::AlwaysSum, &[0, 1]), ["a", "a", "a", "b"]);
    assert_eq!(run(IntersectOp::AlwaysUnion, &[0, 1]), ["a", "a", "b"]);
    // Triple: AsPaper unions at step 3 ({a:2}); AlwaysSum sums ({a:3}).
    assert_eq!(run(IntersectOp::AsPaper, &[0, 1, 2]), ["a", "a"]);
    assert_eq!(run(IntersectOp::AlwaysSum, &[0, 1, 2]), ["a", "a", "a"]);
    assert_eq!(run(IntersectOp::AlwaysUnion, &[0, 1, 2]), ["a", "a"]);
}

#[test]
fn rerank_sort_and_empty_query() {
    let fz = TokenFeaturizer;
    let q = fq(&flat(&["a", "b"]), &fz);
    let cands = vec![
        (MethodId(3), 1, flat(&["a", "z"])),
        (MethodId(1), 1, flat(&["a", "b"])),
        (MethodId(2), 1, flat(&["a", "b", "c"])),
        (MethodId(0), 5, flat(&["a", "y"])),
    ];
    let n2 = rerank(&q, cands.clone(), &fz);
    assert_eq!(n2.iter().map(|r| r.id.0).collect::<Vec<_>>(), [1, 2, 0, 3]);
    assert_eq!(n2[0].score, 1.0);
    assert_eq!(n2[2].score, 0.5);

    let empty = rerank(&sillage_core::FeatureBag::new(), cands, &fz);
    assert!(empty.iter().all(|r| r.score == 0.0 && r.sim == 0));
}

/// Stage 3: a whole regrouped call dropped after `.` (too big to restore) gets
/// its callee back, so the kept receiver never renders as a dangling `view.`.
#[test]
fn complete_calls_restores_member_head_after_dot() {
    use sillage_core::{Lang, NodeKind};
    let text = "void f(View view) {\n  view.setOnTouchListener(new OnTouchListener() {\n    public boolean onTouch(View v, MotionEvent e) {\n      hide(v);\n      return false;\n    }\n  });\n}";
    let body = sillage_core::Span::new(text.find('{').unwrap() as u32, text.len() as u32);
    let base = sillage_parse::parse_method(Lang::Java, text, body).unwrap();
    let toks: Vec<&str> = base
        .leaves
        .iter()
        .map(|&l| match &base.node(l).kind {
            NodeKind::Token { text, .. } => &**text,
            _ => "",
        })
        .collect();
    let keep: Vec<bool> = toks.iter().map(|&t| t == "view").collect();
    let rec = base.retain(&keep);
    let done = sillage_engine::complete_calls(&rec, &base);
    let kept: Vec<&str> = done
        .leaves
        .iter()
        .map(|&l| match &done.node(l).kind {
            NodeKind::Token { text, .. } => &**text,
            _ => "",
        })
        .collect();
    assert_eq!(kept, ["view", "setOnTouchListener"], "only the head of the dropped call");
    // A small hole is still restored whole, and nothing is added when nothing is missing.
    let full = sillage_engine::complete_calls(&base, &base);
    assert_eq!(full, base);
}
