//! §5 Recall@1 / Recall@100 micro-benchmark (Table 4).

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use rayon::prelude::*;
use sillage_core::{FeatureBag, Lang, MethodId};
use sillage_engine::Engine;

use crate::bootstrap::bootstrap_hits;
use crate::sample::{
    MIN_BODY_LINES, QUERY_LINES, code_lines, contiguous_from_lines, noncontiguous_from_lines, sample_methods,
};
use crate::{RecallCfg, RecallMode, RecallReport, RecallRow};

pub const SYSTEM_BM25: &str = "BM25 Keywords";
pub const SYSTEM_FEATURES: &str = "Features Search";
pub const SYSTEM_SILLAGE: &str = "Sillage";

/// Result list depth used for Recall@100 (= paper's η2).
pub const DEPTH: usize = 100;

/// One sampled query: the original method and its partial snippet.
#[derive(Debug, Clone)]
pub struct RecallQuery {
    pub orig: MethodId,
    pub lang: Lang,
    pub snippet: String,
}

/// Per-query, per-system outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hits {
    /// Strict Recall@1: the original is ranked first.
    pub strict1: bool,
    /// §5 Recall@1: strict, or one of Decision 9's "100% similar" cases
    /// (see [`hits`] and [`full_query_tie`]).
    pub r1: bool,
    /// Recall@100: the original is in the top [`DEPTH`].
    pub r100: bool,
}

/// Hits for a ranked id list. `top_same_as_orig` says whether the rank-1
/// method is 100% similar to the original, i.e. has exactly the original's
/// feature multiset `F(orig)` (a duplicate the corpus dedup did not catch, such
/// as a renamed copy).
///
/// Decision 9 (§5): "Aroma should rank the original method body as number 1 in
/// the reranked search result list, or the original method body should be 100%
/// similar to the first code snippet in the ranked results." The clause is a
/// property of the ground truth, not of the system, so it is applied to every
/// system alike. A partial tie (equal pruned similarity, different methods)
/// earns no credit.
pub fn hits(orig: MethodId, results: &[MethodId], top_same_as_orig: bool) -> Hits {
    let strict1 = results.first() == Some(&orig);
    let r100 = results.iter().take(DEPTH).any(|&id| id == orig);
    let r1 = strict1 || (!results.is_empty() && top_same_as_orig);
    Hits { strict1, r1, r100 }
}

/// Whether the original is in a reranked `(id, pruned sim)` list, tied with
/// rank 1, at the full query similarity `|F(q)|` (`query_total`): both methods
/// contain the whole query, so the query cannot tell them apart. Only Sillage
/// has such a score; partial ties (`sim < |F(q)|`) do not count.
pub fn full_query_tie(orig: MethodId, results: &[(MethodId, u64)], query_total: u64) -> bool {
    let Some(&(_, top)) = results.first() else { return false };
    query_total > 0
        && top == query_total
        && results.iter().take(DEPTH).any(|&(id, sim)| id == orig && sim == top)
}

/// [`hits`] with no duplicate credit (strict R@1 in both R@1 fields).
pub fn id_hits(orig: MethodId, results: &[MethodId]) -> Hits {
    hits(orig, results, false)
}

/// Whether `results[0]` is a different method with exactly `F(orig)`;
/// `f_orig` is computed at most once per query.
fn top_is_duplicate(engine: &Engine, orig: MethodId, results: &[MethodId], f_orig: &mut Option<Option<FeatureBag>>) -> bool {
    let Some(&top) = results.first() else { return false };
    if top == orig {
        return false;
    }
    let fo = f_orig.get_or_insert_with(|| engine.method_features(orig));
    fo.as_ref().is_some_and(|fo| engine.method_features(top).as_ref() == Some(fo))
}

/// Builds the sampled query set deterministically from `cfg`.
pub fn build_queries(engine: &Engine, cfg: &RecallCfg) -> Vec<RecallQuery> {
    let methods = &engine.corpus.methods;
    let picks = sample_methods(methods, cfg.n, MIN_BODY_LINES, cfg.seed);
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed ^ 0x5111_a6e0_0000_0005);
    picks
        .into_iter()
        .filter_map(|i| {
            let m = &methods[i];
            let lines = code_lines(m);
            let snippet = match cfg.mode {
                RecallMode::Contiguous => contiguous_from_lines(&lines, QUERY_LINES),
                RecallMode::NonContiguous => noncontiguous_from_lines(&lines, QUERY_LINES, &mut rng),
            };
            (!snippet.trim().is_empty()).then(|| RecallQuery {
                orig: m.id,
                lang: m.lang,
                snippet,
            })
        })
        .collect()
}

/// Per-query hits `[bm25, features, sillage]`.
/// Engine errors (e.g. unparsable snippets) count as misses.
pub fn evaluate_query(engine: &Engine, q: &RecallQuery) -> [Hits; 3] {
    let mut f_orig = None;
    let mut score = |ids: &[MethodId]| {
        let dup = top_is_duplicate(engine, q.orig, ids, &mut f_orig);
        hits(q.orig, ids, dup)
    };
    let bm25 = score(&engine.bm25_search(q.lang, &q.snippet, DEPTH));
    let feats = engine.features_only(q.lang, &q.snippet, DEPTH).map(|ids| score(&ids)).unwrap_or_default();
    let sillage = engine
        .query_features(q.lang, &q.snippet)
        .map(|fq| {
            let rs = engine.search_features(q.lang, &fq, DEPTH);
            let pairs: Vec<(MethodId, u64)> = rs.iter().map(|r| (r.id, r.sim)).collect();
            let ids: Vec<MethodId> = pairs.iter().map(|p| p.0).collect();
            let mut h = score(&ids);
            h.r1 |= full_query_tie(q.orig, &pairs, fq.total());
            h
        })
        .unwrap_or_default();
    [bm25, feats, sillage]
}

/// Aggregates per-system hits into a Table 4 row with bootstrap CIs.
pub fn summarize(system: &str, hits: &[Hits], n_resamples: usize, seed: u64) -> RecallRow {
    let col = |f: fn(&Hits) -> bool| -> Vec<bool> { hits.iter().map(f).collect() };
    RecallRow {
        system: system.to_string(),
        recall_at_1: bootstrap_hits(&col(|h| h.r1), n_resamples, seed),
        recall_at_100: bootstrap_hits(&col(|h| h.r100), n_resamples, seed.wrapping_add(1)),
        strict_recall_at_1: bootstrap_hits(&col(|h| h.strict1), n_resamples, seed.wrapping_add(2)),
    }
}

pub fn run(engine: &Engine, cfg: RecallCfg) -> anyhow::Result<RecallReport> {
    let queries = build_queries(engine, &cfg);
    let per_query: Vec<[Hits; 3]> = queries.par_iter().map(|q| evaluate_query(engine, q)).collect();
    let column = |k: usize| -> Vec<Hits> { per_query.iter().map(|r| r[k]).collect() };
    let rows = [SYSTEM_BM25, SYSTEM_FEATURES, SYSTEM_SILLAGE]
        .iter()
        .enumerate()
        .map(|(k, name)| summarize(name, &column(k), cfg.bootstrap, cfg.seed.wrapping_add(17 * k as u64)))
        .collect();
    Ok(RecallReport {
        mode: cfg.mode,
        sampled: queries.len(),
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: MethodId = MethodId(1);
    const B: MethodId = MethodId(2);
    const C: MethodId = MethodId(3);

    fn h(strict1: bool, r1: bool, r100: bool) -> Hits {
        Hits { strict1, r1, r100 }
    }

    #[test]
    fn id_hits_top1_and_top100() {
        assert_eq!(id_hits(A, &[A, B]), h(true, true, true));
        assert_eq!(id_hits(A, &[B, A]), h(false, false, true));
        assert_eq!(id_hits(A, &[B, C]), h(false, false, false));
        assert_eq!(id_hits(A, &[]), h(false, false, false));
        let mut long: Vec<MethodId> = (10..200).map(MethodId).collect();
        long.insert(150, A);
        assert_eq!(id_hits(A, &long), h(false, false, false));
    }

    #[test]
    fn decision_9_duplicate_clause() {
        // Rank-1 is a different method with exactly F(orig) → §5 hit, strict miss.
        assert_eq!(hits(A, &[B, A], true), h(false, true, true));
        // A mere tie (or anything else) is not a duplicate → miss@1.
        assert_eq!(hits(A, &[B, A], false), h(false, false, true));
        // The original first is always a hit.
        assert_eq!(hits(A, &[A, B], false), h(true, true, true));
        // No results → no credit even if flagged.
        assert_eq!(hits(A, &[], true), h(false, false, false));
        // Duplicate at rank 1 with the original beyond the depth.
        assert_eq!(hits(A, &[B], true), h(false, true, false));
    }

    #[test]
    fn decision_9_full_query_tie() {
        // |F(q)| = 10: tied at rank 1 with the whole query → credit.
        assert!(full_query_tie(A, &[(B, 10), (A, 10)], 10));
        // Partial tie (8 < |F(q)|) → no credit.
        assert!(!full_query_tie(A, &[(B, 8), (A, 8)], 10));
        // Not tied, absent, empty query → no credit.
        assert!(!full_query_tie(A, &[(B, 10), (A, 9)], 10));
        assert!(!full_query_tie(A, &[(B, 10), (C, 10)], 10));
        assert!(!full_query_tie(A, &[(B, 0), (A, 0)], 0));
        assert!(!full_query_tie(A, &[], 10));
    }

    #[test]
    fn summarize_builds_row() {
        let hits = vec![h(true, true, true), h(false, false, true), h(false, true, true), h(false, false, false)];
        let row = summarize(SYSTEM_SILLAGE, &hits, 200, 7);
        assert_eq!(row.system, "Sillage");
        assert_eq!(row.recall_at_1.mean, 0.5);
        assert_eq!(row.strict_recall_at_1.mean, 0.25);
        assert_eq!(row.recall_at_100.mean, 0.75);
        assert!(row.recall_at_1.low <= 0.5 && row.recall_at_1.high >= 0.5);
    }
}
