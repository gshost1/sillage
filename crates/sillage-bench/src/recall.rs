//! §5 Recall@1 / Recall@100 micro-benchmark (Table 4).

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use rayon::prelude::*;
use sillage_core::{Lang, MethodId};
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
    /// §5 Recall@1: strict, or the original is "100% similar" to the query and
    /// tied with the top result (see [`ranked_hits`]).
    pub r1: bool,
    /// Recall@100: the original is in the top [`DEPTH`].
    pub r100: bool,
}

/// Hits for a plain ranked id list (the BM25 and feature-search baselines).
/// These systems have no similarity score comparable to `|F(q)|`, so their
/// §5 Recall@1 is the strict one: ties are broken by `MethodId` ascending and
/// get no credit.
pub fn id_hits(orig: MethodId, results: &[MethodId]) -> Hits {
    let strict1 = results.first() == Some(&orig);
    let r100 = results.iter().take(DEPTH).any(|&id| id == orig);
    Hits { strict1, r1: strict1, r100 }
}

/// Hits for a reranked `(id, pruned sim)` list, where `query_total = |F(q)|`.
///
/// Decision 9 (§5's "100% similar" clause): Recall@1 also counts a hit when the
/// original is in the list, ties the top result, *and* that shared similarity
/// is the maximum `|F(q)|`, i.e. the original and the rank-1 method both contain
/// the whole query and Sillage cannot tell them apart. A partial tie
/// (`sim < |F(q)|`) is a miss, exactly as in strict Recall@1.
pub fn ranked_hits(orig: MethodId, results: &[(MethodId, u64)], query_total: u64) -> Hits {
    let found = results.iter().take(DEPTH).find(|(id, _)| *id == orig);
    let strict1 = results.first().is_some_and(|&(id, _)| id == orig);
    let tie_full = match (results.first(), found) {
        (Some(&(_, top)), Some(&(_, sim))) => query_total > 0 && top == query_total && sim == top,
        _ => false,
    };
    Hits { strict1, r1: strict1 || tie_full, r100: found.is_some() }
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
    let bm25 = id_hits(q.orig, &engine.bm25_search(q.lang, &q.snippet, DEPTH));
    let feats = engine
        .features_only(q.lang, &q.snippet, DEPTH)
        .map(|ids| id_hits(q.orig, &ids))
        .unwrap_or_default();
    let sillage = engine
        .query_features(q.lang, &q.snippet)
        .map(|fq| {
            let rs = engine.search_features(q.lang, &fq, DEPTH);
            let pairs: Vec<(MethodId, u64)> = rs.iter().map(|r| (r.id, r.sim)).collect();
            ranked_hits(q.orig, &pairs, fq.total())
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
    fn ranked_hits_decision_9_tie_clause() {
        // |F(q)| = 10.
        assert_eq!(ranked_hits(A, &[(A, 10), (B, 10)], 10), h(true, true, true));
        // Original ranked 2nd but 100% similar and tied with rank 1 → §5 hit, strict miss.
        assert_eq!(ranked_hits(A, &[(B, 10), (A, 10)], 10), h(false, true, true));
        // Stage 5: a *partial* tie (sim 8 < |F(q)| = 10) earns no §5 credit.
        assert_eq!(ranked_hits(A, &[(B, 8), (A, 8)], 10), h(false, false, true));
        // Original ranked 2nd and strictly less similar → miss@1.
        assert_eq!(ranked_hits(A, &[(B, 10), (A, 9)], 10), h(false, false, true));
        // Original absent → miss.
        assert_eq!(ranked_hits(A, &[(B, 10), (C, 10)], 10), h(false, false, false));
        // Empty query never earns tie credit.
        assert_eq!(ranked_hits(A, &[(B, 0), (A, 0)], 0), h(false, false, true));
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
