use serde::{Deserialize, Serialize};

/// Denominator choice for the cluster similarity constraint `s(τ) > τ3` (Section 3.3.3).
///
/// The paper's printed formula writes `s(τ) = csq(τ) / |F(N2(i1))|`. Because
/// `cs(τ) <= |F(N2(i1))|` and `l(τ) = cs(τ) / csq(τ) > 1.5`, the printed formula
/// implies `s(τ) < 1 / 1.5 ≈ 0.67`, making `s(τ) > 0.9` mathematically impossible.
/// The accompanying prose states that the intersection of pruned snippets in a cluster
/// should be "very similar to the first *pruned* snippet", so [`SDenom::PrunedFirst`]
/// (`|F(Prune(F(q), N2(i1)))|`) is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SDenom {
    PrunedFirst,
    PaperLiteral,
}

/// Feature combination operator inside `Intersect(τ, q)` (Section 3.3.3).
///
/// The paper uses multiset sum (`⊎`) for the 2-element base case and multiset union
/// (`∪`, elementwise max) for 3+ elements (`AsPaper`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntersectOp {
    AsPaper,
    AlwaysSum,
    AlwaysUnion,
}

/// Tuple deduplication strategy for `N3` (Section 3.3.3).
///
/// `AllEarlier` compares Jaccard similarity against every earlier tuple in the sorted
/// list (literal reading of the paper). `KeptOnly` compares only against retained tuples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dedup {
    AllEarlier,
    KeptOnly,
}

/// Algorithm hyperparameters and paper disambiguation toggles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    /// Phase I candidate count (paper default: 1000).
    pub eta1: usize,
    /// Phase III candidate cap from reranked N2 (paper default: 100).
    pub eta2: usize,
    /// Minimum normalized pruned query similarity `SimScore(q, Prune(q, m)) / |F(q)|` (paper default: 0.65).
    pub tau1: f64,
    /// Minimum cluster extension ratio `l(τ) = cs(τ) / csq(τ)` (paper default: 1.5).
    pub tau2: f64,
    /// Minimum cluster query-alignment similarity `s(τ)` (paper default: 0.9).
    pub tau3: f64,
    /// Maximum allowed Jaccard similarity between cluster index tuples (paper default: 0.5).
    pub jaccard: f64,
    /// Maximum number of recommendations returned (paper default: 5).
    pub k: usize,
    pub s_denominator: SDenom,
    pub intersect_union: IntersectOp,
    pub dedup_against: Dedup,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            eta1: 1000,
            eta2: 100,
            tau1: 0.65,
            tau2: 1.5,
            tau3: 0.9,
            jaccard: 0.5,
            k: 5,
            s_denominator: SDenom::PrunedFirst,
            intersect_union: IntersectOp::AsPaper,
            dedup_against: Dedup::AllEarlier,
        }
    }
}
