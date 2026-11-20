//! Paper Table 2: features of `view_30` and `0_20` in the Fig. 4 tree.

mod common;

use common::{build, g, k, leaf_index, listing4, n, v};
use sillage_core::Featurizer;
use sillage_index::{AromaFeaturizer, feature_debug};

#[test]
fn table2_view30() {
    let tree = listing4();
    // view_6 is the first `view`, view_30 the second (in the loop condition).
    let idx = leaf_index(&tree, "view", 1);
    assert_eq!(
        feature_debug(&tree, idx),
        [
            "#VAR",
            "(#VAR, 2, (#)#)",
            "(#VAR, 1, (#))",
            "(#VAR, 1, #.#)",
            "(ViewGroup, #VAR)",
            "(#VAR, getChildCount)",
            "((1, #instanceof#), (2, (#)#))",
            "((2, (#)#), (2, (#)#))",
        ]
    );
}

#[test]
fn table2_zero20() {
    let tree = listing4();
    let idx = leaf_index(&tree, "0", 0);
    // Table 2 prints the third parent as `#;#;#;`; Fig. 4 (and Def. 4 applied to
    // `init ; cond ; update`) gives `#;#;#`, which is what we assert.
    assert_eq!(
        feature_debug(&tree, idx),
        ["0", "(0, 2, #=#)", "(0, 1, int#)", "(0, 1, #;#;#)", "(#VAR, 0)", "(0, #VAR)"]
    );
}

#[test]
fn first_and_last_leaves_have_one_sibling() {
    let tree = listing4();
    // view_6: no Prev sibling, no PrevUse.
    assert_eq!(
        feature_debug(&tree, 0),
        [
            "#VAR",
            "(#VAR, 1, #instanceof#)",
            "(#VAR, 1, (#))",
            "(#VAR, 1, if##)",
            "(#VAR, ViewGroup)",
            "((1, #instanceof#), (2, (#)#))",
        ]
    );
    // i_35: no Next sibling, no NextUse; PrevUse is the `i` in `i++`.
    assert_eq!(
        feature_debug(&tree, tree.leaves.len() - 1),
        [
            "#VAR",
            "(#VAR, 2, #(#))",
            "(#VAR, 2, #.#)",
            "(#VAR, 2, #=#)",
            "(getChildAt, #VAR)",
            "((1, #++), (2, #(#)))",
        ]
    );
}

#[test]
fn member_context_for_receiver() {
    // `x.foo(); x = 1;` → C(x_1) = foo (parent is `#.#`), C(x_2) = (1, #=#).
    let tree = build(n(vec![
        n(vec![n(vec![v("x", 0), k("."), n(vec![g("foo"), k("("), k(")")])]), k(";")]),
        n(vec![n(vec![v("x", 0), k("="), g("1")]), k(";")]),
    ]));
    let f = feature_debug(&tree, 0);
    assert_eq!(f.last().unwrap(), "(foo, (1, #=#))");
    let f = feature_debug(&tree, leaf_index(&tree, "x", 1));
    assert_eq!(f.last().unwrap(), "(foo, (1, #=#))");
}

#[test]
fn member_context_falls_back_to_slot() {
    // `a.b` with both locals: no non-local token under `#.#` → `(i, #.#)`.
    let tree = build(n(vec![n(vec![v("a", 0), k("."), v("b", 1)]), k("+"), n(vec![v("a", 0), k("."), v("b", 1)])]));
    let f = feature_debug(&tree, 0);
    assert_eq!(f.last().unwrap(), "((1, #.#), (1, #.#))");
}

#[test]
fn featurize_matches_debug_and_is_alpha_invariant() {
    let tree = listing4();
    let feats = AromaFeaturizer.featurize(&tree);
    assert_eq!(feats.per_leaf.len(), tree.leaves.len());
    for (i, bag) in feats.per_leaf.iter().enumerate() {
        assert_eq!(bag.total() as usize, feature_debug(&tree, i).len());
    }

    // Renaming locals must not change any feature.
    let rename = |a: &'static str, b: &'static str| {
        build(n(vec![
            n(vec![n(vec![v(a, 0), k("="), g("0")]), k(";")]),
            n(vec![n(vec![v(b, 1), k("="), n(vec![v(a, 0), k("+"), g("1")])]), k(";")]),
        ]))
    };
    let x = AromaFeaturizer.featurize(&rename("x", "y")).total();
    let y = AromaFeaturizer.featurize(&rename("p", "q")).total();
    assert_eq!(x, y);

    // ...but swapping a global does.
    let other = build(n(vec![
        n(vec![n(vec![v("x", 0), k("="), g("0")]), k(";")]),
        n(vec![n(vec![v("y", 1), k("="), n(vec![v("x", 0), k("+"), g("2")])]), k(";")]),
    ]));
    assert_ne!(x, AromaFeaturizer.featurize(&other).total());
}

#[test]
fn out_of_range_leaf_is_empty() {
    let tree = listing4();
    assert!(feature_debug(&tree, tree.leaves.len()).is_empty());
}

/// Stage 3 query mode: sibling and variable-usage features only link leaves on
/// the same line, and no parent feature names a statement-sequence root;
/// methods keep every feature.
#[test]
fn query_mode_drops_cross_line_relations() {
    use sillage_core::Lang;
    use sillage_index::query_feature_debug;
    let tree = sillage_parse::parse_snippet(Lang::Java, "int n = count(4);\nverify(n);").unwrap();
    let all = |f: fn(&sillage_core::SimplifiedTree, usize) -> Vec<String>| -> Vec<String> {
        (0..tree.leaves.len()).flat_map(|i| f(&tree, i)).collect()
    };
    let (full, query) = (all(feature_debug), all(query_feature_debug));
    // Cross-line sibling `(4, verify)` and the usage pair of `n` across lines.
    assert!(full.iter().any(|f| f == "(4, verify)"), "{full:?}");
    assert!(!query.iter().any(|f| f == "(4, verify)"), "{query:?}");
    let usage = |fs: &[String]| fs.iter().filter(|f| f.starts_with("((")).count();
    assert!(usage(&full) > 0 && usage(&query) == 0, "{full:?}\n{query:?}");
    // Same-line siblings stay; root `{#}` parents go.
    assert!(query.iter().any(|f| f == "(count, 4)"), "{query:?}");
    assert!(full.iter().any(|f| f.ends_with(", 0, {#})")));
    assert!(!query.iter().any(|f| f.ends_with(", 0, {#})")), "{query:?}");
    // The trait method agrees with the debug view.
    let q = AromaFeaturizer.featurize_query(&tree).total().total();
    assert_eq!(q, query.len() as u64);
    assert!(AromaFeaturizer.featurize(&tree).total().total() > q);
}
