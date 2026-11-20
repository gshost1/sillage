//! Phase I index vs. a brute-force set-intersection oracle.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use proptest::prelude::*;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use sillage_core::{
    FeatureBag, FeatureId, Featurizer, Lang, LeafFeatures, MethodId, Node, NodeKind, SimplifiedTree, Span,
};
use sillage_index::{AromaFeaturizer, InvertedIndex};

/// Featurizer whose feature ID is each leaf's text parsed as a number.
struct NumFeaturizer;

impl Featurizer for NumFeaturizer {
    fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures {
        LeafFeatures {
            per_leaf: tree
                .leaves
                .iter()
                .map(|&l| FeatureBag::from_unsorted([tree.label(l).parse::<FeatureId>().unwrap()]))
                .collect(),
        }
    }
}

/// Flat tree (root + one token per feature); structural validity is irrelevant here.
fn flat_tree(ids: &[FeatureId]) -> SimplifiedTree {
    let mut nodes = vec![Node {
        kind: NodeKind::Internal { label: "#".repeat(ids.len()).into() },
        parent: None,
        index_in_parent: 0,
        children: (1..=ids.len() as u32).collect(),
        span: Span::default(),
        dropped_children: false,
    }];
    for (i, id) in ids.iter().enumerate() {
        nodes.push(Node {
            kind: NodeKind::Token { text: id.to_string().into(), var: None },
            parent: Some(0),
            index_in_parent: i as u16 + 1,
            children: Vec::new(),
            span: Span::default(),
            dropped_children: false,
        });
    }
    SimplifiedTree {
        lang: Lang::Java,
        source: Arc::from(""),
        nodes,
        leaves: (1..=ids.len() as u32).collect(),
    }
}

fn oracle(methods: &[Vec<FeatureId>], query: &[FeatureId], eta1: usize) -> Vec<(MethodId, u32)> {
    let q: BTreeSet<_> = query.iter().copied().collect();
    let mut hits: Vec<(MethodId, u32)> = methods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let s: BTreeSet<_> = m.iter().copied().collect();
            (MethodId(i as u32), s.intersection(&q).count() as u32)
        })
        .filter(|&(_, c)| c > 0)
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    hits.truncate(eta1);
    hits
}

fn build(methods: &[Vec<FeatureId>]) -> InvertedIndex {
    let trees: Vec<SimplifiedTree> = methods.iter().map(|m| flat_tree(m)).collect();
    let refs: Vec<(MethodId, &SimplifiedTree)> =
        trees.iter().enumerate().map(|(i, t)| (MethodId(i as u32), t)).collect();
    InvertedIndex::build_from_trees(&refs, methods.len(), &NumFeaturizer)
}

proptest! {
    #[test]
    fn search_matches_oracle(
        methods in prop::collection::vec(prop::collection::vec(0u64..40, 0..25), 0..30),
        query in prop::collection::vec(0u64..50, 0..30),
        eta1 in 0usize..40,
    ) {
        let index = build(&methods);
        let bag = FeatureBag::from_unsorted(query.iter().copied());
        prop_assert_eq!(index.search(&bag, eta1), oracle(&methods, &query, eta1));
        prop_assert_eq!(index.num_methods(), methods.len());
        let uniq: BTreeSet<_> = methods.iter().flatten().copied().collect();
        prop_assert_eq!(index.num_features(), uniq.len());
        let postings: usize = methods.iter().map(|m| m.iter().collect::<BTreeSet<_>>().len()).sum();
        prop_assert_eq!(index.num_postings(), postings);
    }
}

#[test]
fn search_matches_oracle_large_seeded() {
    let mut rng = ChaCha8Rng::seed_from_u64(0x5111_a6e);
    let methods: Vec<Vec<FeatureId>> = (0..2000)
        .map(|_| {
            let len = rng.random_range(0..80);
            (0..len).map(|_| rng.random_range(0..600u64)).collect()
        })
        .collect();
    let index = build(&methods);
    for eta1 in [1, 10, 100, 1000, 5000] {
        let query: Vec<FeatureId> = (0..rng.random_range(1..60)).map(|_| rng.random_range(0..700u64)).collect();
        let bag = FeatureBag::from_unsorted(query.iter().copied());
        assert_eq!(index.search(&bag, eta1), oracle(&methods, &query, eta1), "eta1 = {eta1}");
    }
}

#[test]
fn set_semantics_ignore_counts() {
    // Method 0 repeats feature 1 many times; method 1 has {1, 2}.
    let index = build(&[vec![1; 50], vec![1, 2]]);
    let q = FeatureBag::from_weighted([(1, 7), (2, 3)]);
    assert_eq!(index.search(&q, 10), [(MethodId(1), 2), (MethodId(0), 1)]);
}

#[test]
fn edge_cases() {
    let index = build(&[vec![1, 2], vec![], vec![3]]);
    assert!(index.search(&FeatureBag::new(), 10).is_empty());
    assert!(index.search(&FeatureBag::from_unsorted([1]), 0).is_empty());
    assert!(index.search(&FeatureBag::from_unsorted([99]), 10).is_empty());
    assert_eq!(index.num_methods(), 3);

    let empty = InvertedIndex::default();
    assert!(empty.search(&FeatureBag::from_unsorted([1]), 5).is_empty());
    assert_eq!((empty.num_features(), empty.num_postings(), empty.num_methods()), (0, 0, 0));
}

#[test]
fn duplicate_ids_merge_and_num_methods_grows() {
    let a = flat_tree(&[1, 2]);
    let b = flat_tree(&[2, 3]);
    let index = InvertedIndex::build_from_trees(&[(MethodId(5), &a), (MethodId(5), &b)], 2, &NumFeaturizer);
    assert_eq!(index.num_methods(), 6);
    assert_eq!(index.num_postings(), 3);
    assert_eq!(index.search(&FeatureBag::from_unsorted([1, 2, 3]), 5), [(MethodId(5), 3)]);
}

#[test]
fn aroma_query_ranks_itself_first() {
    let full = common::listing4();
    let other = common::build(common::n(vec![
        common::n(vec![common::v("x", 0), common::k("="), common::g("0")]),
        common::k(";"),
    ]));
    let index = InvertedIndex::build_from_trees(
        &[(MethodId(0), &other), (MethodId(1), &full)],
        2,
        &AromaFeaturizer,
    );
    let q = AromaFeaturizer.featurize(&full).total();
    let hits = index.search(&q, 10);
    assert_eq!(hits[0], (MethodId(1), q.set_len() as u32));
}
