//! Paper running example (§3.3): Listing 4 query, Listings 5/6 + distractors → Listing 7.
mod common;

use common::*;
use sillage_core::{MethodId, Params, SDenom, SimplifiedTree};
use sillage_engine::{cluster, cluster_scored, intersect, prune, rerank};

fn paren(x: B) -> B {
    n(vec![k("("), x, k(")")])
}
fn block(stmts: Vec<B>) -> B {
    let mut v = vec![k("{")];
    v.extend(stmts);
    v.push(k("}"));
    n(v)
}
fn stmt(x: B) -> B {
    n(vec![x, k(";")])
}
/// `recv . name ( args )`
fn call(recv: B, name: &'static str, args: Vec<B>) -> B {
    let mut v = vec![recv, k("."), t(name), k("(")];
    for (i, a) in args.into_iter().enumerate() {
        if i > 0 {
            v.push(k(","));
        }
        v.push(a);
    }
    v.push(k(")"));
    n(v)
}
/// `( ( ViewGroup ) view )`
fn cast_view() -> B {
    paren(n(vec![k("("), t("ViewGroup"), k(")"), v("view")]))
}

/// `if (view instanceof ViewGroup) { for (...) { View innerView = ...; [extra(innerView);] } }`
fn if_viewgroup(extra: Option<&'static str>) -> B {
    let mut body = vec![n(vec![
        t("View"),
        n(vec![v("innerView"), k("="), call(cast_view(), "getChildAt", vec![v("i")])]),
        k(";"),
    ])];
    if let Some(name) = extra {
        body.push(stmt(n(vec![t(name), k("("), v("innerView"), k(")")])));
    }
    let for_stmt = n(vec![
        k("for"),
        k("("),
        n(vec![k("int"), n(vec![v("i"), k("="), t("0")]), k(";")]),
        n(vec![v("i"), k("<"), call(cast_view(), "getChildCount", vec![])]),
        k(";"),
        n(vec![v("i"), k("++")]),
        k(")"),
        block(body),
    ]);
    n(vec![
        k("if"),
        paren(n(vec![v("view"), k("instanceof"), t("ViewGroup")])),
        block(vec![for_stmt]),
    ])
}

/// `if (!(view instanceof EditText)) { view.setOnTouchListener(new View.OnTouchListener() {
///     public boolean onTouch(View v, MotionEvent event) { BODY; return false; } }); }`
fn if_touch(body: B) -> B {
    let method = n(vec![
        k("public"),
        k("boolean"),
        t("onTouch"),
        n(vec![
            k("("),
            n(vec![t("View"), v("v")]),
            k(","),
            n(vec![t("MotionEvent"), v("event")]),
            k(")"),
        ]),
        block(vec![body, n(vec![k("return"), t("false"), k(";")])]),
    ]);
    let anon = n(vec![
        k("new"),
        n(vec![t("View"), k("."), t("OnTouchListener")]),
        k("("),
        k(")"),
        block(vec![method]),
    ]);
    n(vec![
        k("if"),
        paren(n(vec![k("!"), paren(n(vec![v("view"), k("instanceof"), t("EditText")]))])),
        block(vec![stmt(call(v("view"), "setOnTouchListener", vec![anon]))]),
    ])
}

fn listing4() -> SimplifiedTree {
    build(if_viewgroup(None))
}
fn listing5() -> SimplifiedTree {
    let hide = stmt(n(vec![t("hideKeyBoard"), k("("), k(")")]));
    build(block(vec![if_touch(hide), if_viewgroup(Some("setupUIToHideKeyBoardOnTouch"))]))
}
fn listing6() -> SimplifiedTree {
    let toggle = stmt(call(
        t("Utils"),
        "toggleSoftKeyBoard",
        vec![n(vec![t("LoginActivity"), k("."), k("this")]), t("true")],
    ));
    build(block(vec![if_touch(toggle), if_viewgroup(Some("setupUI"))]))
}
/// Contains the query plus unrelated extra code.
fn distractor_a() -> SimplifiedTree {
    build(block(vec![
        stmt(call(t("Log"), "d", vec![t("TAG"), t("\"layout\"")])),
        if_viewgroup(Some("applyTheme")),
        stmt(n(vec![v("count"), k("++")])),
    ]))
}
/// The query plus one tiny call: never a valid cluster on its own (l ≈ 1).
fn distractor_b() -> SimplifiedTree {
    build(block(vec![if_viewgroup(Some("refresh"))]))
}
/// Unrelated code: falls below τ1.
fn distractor_c() -> SimplifiedTree {
    let s = n(vec![v("sum"), k("+="), n(vec![v("xs"), k("["), v("j"), k("]")])]);
    build(block(vec![stmt(s), n(vec![k("return"), v("sum"), k(";")])]))
}

fn n2_for(params_fq: &sillage_core::FeatureBag) -> Vec<sillage_engine::Ranked> {
    let cands = vec![
        (MethodId(5), 50, listing5()),
        (MethodId(6), 49, listing6()),
        (MethodId(10), 48, distractor_a()),
        (MethodId(11), 47, distractor_b()),
        (MethodId(12), 3, distractor_c()),
    ];
    rerank(params_fq, params_fq, cands, &TestFeaturizer)
}

#[test]
fn rerank_orders_and_scores() {
    let fz = TestFeaturizer;
    let q = fq(&listing4(), &fz);
    let n2 = n2_for(&q);
    let ids: Vec<u32> = n2.iter().map(|r| r.id.0).collect();
    assert_eq!(ids, [5, 6, 10, 11, 12]);
    for r in &n2[..4] {
        assert!(r.score > 0.9, "{} score {}", r.id, r.score);
    }
    assert!(n2[4].score < Params::default().tau1);
    for w in n2.windows(2) {
        assert!((w[0].sim, w[0].overlap) >= (w[1].sim, w[1].overlap));
    }
    // Pruned Listing 5 holds every query leaf and none of Listing 5's extra calls. (Greedy
    // may also keep a stray `view` from the first `if`, as Section 5 of the paper notes.)
    let pruned = leaf_texts(&n2[0].pruned.tree);
    let mut rest = pruned.clone();
    for leaf in leaf_texts(&listing4()) {
        let pos = rest.iter().position(|l| *l == leaf).expect("query leaf kept");
        rest.remove(pos);
    }
    assert!(rest.iter().all(|l| l == "view"), "unexpected extra leaves {rest:?}");
    for gone in ["hideKeyBoard", "setOnTouchListener", "setupUIToHideKeyBoardOnTouch"] {
        assert!(!pruned.iter().any(|l| l == gone));
    }
}

#[test]
fn listing5_and_listing6_cluster_into_listing7() {
    let fz = TestFeaturizer;
    let params = Params::default();
    let q = fq(&listing4(), &fz);
    let n2 = n2_for(&q);

    let scored = cluster_scored(&q, &n2, &params, &fz);
    assert_eq!(scored[0].tuple, vec![0, 1], "clusters: {scored:?}");
    assert!(scored[0].l > params.tau2 && scored[0].s > params.tau3);
    assert_eq!(
        cluster(&q, &n2, &params, &fz),
        scored.iter().map(|c| c.tuple.clone()).collect::<Vec<_>>()
    );
    // Distractors never join Listing 5's cluster.
    assert!(scored.iter().all(|c| !(c.tuple.contains(&0) && c.tuple.len() > 1 && c.tuple != [0, 1])));
    assert!(scored.iter().all(|c| !c.tuple.contains(&3) && !c.tuple.contains(&4)));

    let rec = intersect(&scored[0].tuple, &q, &n2, &params, &fz);
    rec.validate().unwrap();
    let leaves = leaf_texts(&rec);
    for want in [
        "EditText",
        "setOnTouchListener",
        "OnTouchListener",
        "onTouch",
        "MotionEvent",
        "false",
        "ViewGroup",
        "getChildCount",
        "getChildAt",
        "innerView",
    ] {
        assert!(leaves.iter().any(|l| l == want), "missing {want}: {}", text(&rec));
    }
    for gone in ["hideKeyBoard", "Utils", "toggleSoftKeyBoard", "LoginActivity"] {
        assert!(!leaves.iter().any(|l| l == gone), "kept {gone}: {}", text(&rec));
    }
    let src = text(&rec);
    assert!(src.contains("return false ;"), "{src}");
    assert!(src.contains("for ( int i = 0 ;"), "{src}");

    // The query is (almost) fully matched inside the recommendation; relabelled ancestors
    // after pruning can cost a feature or two.
    let qm = prune(&q, &rec, &fz.featurize(&rec));
    assert!(qm.sim * 100 >= q.total() * 95, "{} / {}", qm.sim, q.total());
    eprintln!("Listing 7 (tokens): {src}");
}

#[test]
fn paper_literal_s_denominator_rejects_every_cluster() {
    // Decision 1: with s = csq / |F(N2(i1))|, s < 1/τ2 whenever l > τ2.
    let fz = TestFeaturizer;
    let params = Params { s_denominator: SDenom::PaperLiteral, ..Params::default() };
    let q = fq(&listing4(), &fz);
    let n2 = n2_for(&q);
    assert!(cluster(&q, &n2, &params, &fz).is_empty());
    let relaxed = Params { tau3: 0.3, ..params };
    assert_eq!(cluster(&q, &n2, &relaxed, &fz)[0], vec![0, 1]);
}
