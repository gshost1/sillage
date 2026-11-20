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

/// `(hit@1, hit@100)` for a plain ranked id list.
pub fn id_hits(orig: MethodId, results: &[MethodId]) -> (bool, bool) {
    let r1 = results.first() == Some(&orig);
    let r100 = results.iter().take(DEPTH).any(|&id| id == orig);
    (r1, r100)
}

/// `(hit@1, hit@100)` for a reranked `(id, pruned sim)` list, with Decision 9:
/// Recall@1 also counts a hit when the original is retrieved and its pruned
/// similarity equals the top result's ("100% similar" clause of §5).
pub fn ranked_hits(orig: MethodId, results: &[(MethodId, u64)]) -> (bool, bool) {
    let top = results.iter().take(DEPTH);
    let found = top.clone().find(|(id, _)| *id == orig);
    let r100 = found.is_some();
    let r1 = match (results.first(), found) {
        (Some(&(first, _)), _) if first == orig => true,
        (Some(&(_, top_sim)), Some(&(_, orig_sim))) => top_sim == orig_sim,
        _ => false,
    };
    (r1, r100)
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

/// Per-query hits `[bm25, features, sillage]`, each `(hit@1, hit@100)`.
/// Engine errors (e.g. unparsable snippets) count as misses.
pub fn evaluate_query(engine: &Engine, q: &RecallQuery) -> [(bool, bool); 3] {
    let bm25 = id_hits(q.orig, &engine.bm25_search(&q.snippet, DEPTH));
    let feats = engine
        .features_only(q.lang, &q.snippet, DEPTH)
        .map(|ids| id_hits(q.orig, &ids))
        .unwrap_or((false, false));
    let sillage = engine
        .search(q.lang, &q.snippet, DEPTH)
        .map(|rs| {
            let pairs: Vec<(MethodId, u64)> = rs.iter().map(|r| (r.id, r.sim)).collect();
            ranked_hits(q.orig, &pairs)
        })
        .unwrap_or((false, false));
    [bm25, feats, sillage]
}

/// Aggregates per-system hit vectors into a Table 4 row with bootstrap CIs.
pub fn summarize(system: &str, hits: &[(bool, bool)], n_resamples: usize, seed: u64) -> RecallRow {
    let h1: Vec<bool> = hits.iter().map(|h| h.0).collect();
    let h100: Vec<bool> = hits.iter().map(|h| h.1).collect();
    RecallRow {
        system: system.to_string(),
        recall_at_1: bootstrap_hits(&h1, n_resamples, seed),
        recall_at_100: bootstrap_hits(&h100, n_resamples, seed.wrapping_add(1)),
    }
}

pub fn run(engine: &Engine, cfg: RecallCfg) -> anyhow::Result<RecallReport> {
    let queries = build_queries(engine, &cfg);
    let per_query: Vec<[(bool, bool); 3]> =
        queries.par_iter().map(|q| evaluate_query(engine, q)).collect();
    let column = |k: usize| -> Vec<(bool, bool)> { per_query.iter().map(|r| r[k]).collect() };
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

    #[test]
    fn id_hits_top1_and_top100() {
        assert_eq!(id_hits(A, &[A, B]), (true, true));
        assert_eq!(id_hits(A, &[B, A]), (false, true));
        assert_eq!(id_hits(A, &[B, C]), (false, false));
        assert_eq!(id_hits(A, &[]), (false, false));
        let mut long: Vec<MethodId> = (10..200).map(MethodId).collect();
        long.insert(150, A);
        assert_eq!(id_hits(A, &long), (false, false));
    }

    #[test]
    fn ranked_hits_decision_9_tie_clause() {
        assert_eq!(ranked_hits(A, &[(A, 10), (B, 10)]), (true, true));
        // Original ranked 2nd but equally similar → Recall@1 hit.
        assert_eq!(ranked_hits(A, &[(B, 10), (A, 10)]), (true, true));
        // Original ranked 2nd and strictly less similar → miss@1.
        assert_eq!(ranked_hits(A, &[(B, 12), (A, 10)]), (false, true));
        // Original absent → miss.
        assert_eq!(ranked_hits(A, &[(B, 10), (C, 10)]), (false, false));
    }

    #[test]
    fn summarize_builds_row() {
        let hits = vec![(true, true), (false, true), (true, true), (false, false)];
        let row = summarize(SYSTEM_SILLAGE, &hits, 200, 7);
        assert_eq!(row.system, "Sillage");
        assert_eq!(row.recall_at_1.mean, 0.5);
        assert_eq!(row.recall_at_100.mean, 0.75);
        assert!(row.recall_at_1.low <= 0.5 && row.recall_at_1.high >= 0.5);
    }
}
