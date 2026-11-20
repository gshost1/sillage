//! §4.2 recommendation quality on half-snippet queries.
//!
//! For each sampled method, the first half of its non-empty body lines is the
//! query and the second half is held out. Per recommendation `r`:
//!
//! * containment `|F(q) ∩ F(r)| / |F(q)|`; a query is *contained* when some
//!   recommendation reaches `params.tau1`;
//! * added features `A = F(r) \ F(q)` (multiset difference), scored against
//!   the held-out features `H = F(held)`: precision `|A ∩ H| / |A|`,
//!   recall `|A ∩ H| / |H|`.
//!
//! The best recommendation per query (by F1) is reported. Precision/recall are
//! averaged over queries that produced at least one recommendation.
//! Recommendations whose cluster contains the query's own method are excluded
//! (leave-one-out), since the original is in the indexed corpus.

use std::collections::BTreeSet;
use std::time::Instant;

use sillage_core::{FeatureBag, SimplifiedTree};
use sillage_engine::{Engine, Recommendation, Timings};

use crate::bootstrap::{mean, percentile};
use crate::sample::{code_lines, sample_methods, split_half_from_lines};
use crate::{PhaseLatency, QualityCfg, QualityReport};

/// Minimum non-empty body lines for a quality sample (query gets ≥ 4 lines).
pub const QUALITY_MIN_LINES: usize = 8;

/// Multiset difference `a \ b` (counts `max(0, a_f - b_f)`).
pub fn bag_difference(a: &FeatureBag, b: &FeatureBag) -> FeatureBag {
    FeatureBag::from_weighted(
        a.entries()
            .iter()
            .map(|&(id, c)| (id, c.saturating_sub(b.get(id)))),
    )
}

/// `|F(q) ∩ F(r)| / |F(q)|` (0 when the query has no features).
pub fn containment(fq: &FeatureBag, fr: &FeatureBag) -> f64 {
    let total = fq.total();
    if total == 0 {
        0.0
    } else {
        fq.intersect_count(fr) as f64 / total as f64
    }
}

/// `(precision, recall)` of the features `F(r) \ F(q)` against `held`.
pub fn added_precision_recall(fq: &FeatureBag, fr: &FeatureBag, held: &FeatureBag) -> (f64, f64) {
    let added = bag_difference(fr, fq);
    let inter = added.intersect_count(held) as f64;
    let p = if added.total() == 0 { 0.0 } else { inter / added.total() as f64 };
    let r = if held.total() == 0 { 0.0 } else { inter / held.total() as f64 };
    (p, r)
}

fn f1(p: f64, r: f64) -> f64 {
    if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
}

/// Number of distinct source lines holding a recommended (non-query) token.
pub fn extra_lines(tree: &SimplifiedTree, query_match: &[bool]) -> usize {
    let src = &tree.source;
    let mut lines = BTreeSet::new();
    for (i, &leaf) in tree.leaves.iter().enumerate() {
        if query_match.get(i).copied().unwrap_or(false) {
            continue;
        }
        let start = (tree.node(leaf).span.start as usize).min(src.len());
        lines.insert(src.as_bytes()[..start].iter().filter(|&&b| b == b'\n').count());
    }
    lines.len()
}

/// Per-query outcome.
#[derive(Debug, Clone, Default)]
pub struct QueryOutcome {
    pub contained: bool,
    pub recommendations: usize,
    pub best: Option<(f64, f64)>,
    pub extra_lines: Vec<usize>,
    pub ms: f64,
    pub timings: Timings,
}

/// Scores the kept recommendations of one query.
pub fn score_recommendations(
    engine: &Engine,
    fq: &FeatureBag,
    held: &FeatureBag,
    recs: &[&Recommendation],
) -> QueryOutcome {
    let mut out = QueryOutcome {
        recommendations: recs.len(),
        ..QueryOutcome::default()
    };
    for rec in recs {
        let fr = engine.featurizer.featurize(&rec.tree).total();
        if containment(fq, &fr) >= engine.params.tau1 {
            out.contained = true;
        }
        let pr = added_precision_recall(fq, &fr, held);
        if out.best.is_none_or(|b| f1(pr.0, pr.1) > f1(b.0, b.1)) {
            out.best = Some(pr);
        }
        out.extra_lines.push(extra_lines(&rec.tree, &rec.query_match));
    }
    out
}

/// Aggregates per-query outcomes into a [`QualityReport`].
pub fn aggregate(outcomes: &[QueryOutcome]) -> QualityReport {
    let n = outcomes.len();
    let contained = outcomes.iter().filter(|o| o.contained).count();
    let bests: Vec<(f64, f64)> = outcomes.iter().filter_map(|o| o.best).collect();
    let ps: Vec<f64> = bests.iter().map(|b| b.0).collect();
    let rs: Vec<f64> = bests.iter().map(|b| b.1).collect();
    let recs: Vec<f64> = outcomes.iter().map(|o| o.recommendations as f64).collect();
    let extra: Vec<f64> = outcomes
        .iter()
        .flat_map(|o| o.extra_lines.iter().map(|&e| e as f64))
        .collect();
    let ms: Vec<f64> = outcomes.iter().map(|o| o.ms).collect();
    let max = |xs: &[f64]| xs.iter().copied().fold(0.0_f64, f64::max);
    let phase = |name: &str, get: fn(&Timings) -> f64| {
        let xs: Vec<f64> = outcomes.iter().map(|o| get(&o.timings)).collect();
        PhaseLatency {
            phase: name.to_string(),
            p50_ms: percentile(&xs, 50.0),
            p95_ms: percentile(&xs, 95.0),
            max_ms: max(&xs),
        }
    };
    let phases = vec![
        phase("parse", |t| t.parse_ms),
        phase("phase1", |t| t.phase1_ms),
        phase("phase2", |t| t.phase2_ms),
        phase("phase3", |t| t.phase3_ms),
    ];
    QualityReport {
        sampled: n,
        containment_rate: if n == 0 { 0.0 } else { contained as f64 / n as f64 },
        avg_precision: mean(&ps),
        avg_recall: mean(&rs),
        avg_recommendations: mean(&recs),
        avg_extra_lines: mean(&extra),
        p50_ms: percentile(&ms, 50.0),
        p95_ms: percentile(&ms, 95.0),
        max_ms: max(&ms),
        phases,
    }
}

pub fn run(engine: &Engine, cfg: QualityCfg) -> anyhow::Result<QualityReport> {
    let methods = &engine.corpus.methods;
    let picks = sample_methods(methods, cfg.n, QUALITY_MIN_LINES, cfg.seed);
    let mut outcomes = Vec::with_capacity(picks.len());
    for i in picks {
        let m = &methods[i];
        let (query, held_text) = split_half_from_lines(&code_lines(m));
        if query.trim().is_empty() || held_text.trim().is_empty() {
            continue;
        }
        let (Ok(qt), Ok(ht)) = (
            sillage_parse::parse_snippet(m.lang, &query),
            sillage_parse::parse_snippet(m.lang, &held_text),
        ) else {
            continue;
        };
        let fq = engine.featurizer.featurize(&qt).total();
        let held = engine.featurizer.featurize(&ht).total();
        // Latency is measured sequentially so the percentiles are not skewed by contention.
        let t0 = Instant::now();
        let Ok(output) = engine.recommend(m.lang, &query) else {
            continue;
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if std::env::var_os("SILLAGE_TRACE").is_some() {
            eprintln!(
                "[quality] {:>9.1} ms  parsed={} n2={} recs={}  {}:{}:{} {}",
                ms,
                output.timings.parsed,
                output.n2.len(),
                output.recommendations.len(),
                m.repo,
                m.path,
                m.line_start,
                m.name
            );
        }
        let kept: Vec<&Recommendation> = output
            .recommendations
            .iter()
            .filter(|r| !r.cluster.contains(&m.id))
            .collect();
        let mut o = score_recommendations(engine, &fq, &held, &kept);
        o.ms = ms;
        o.timings = output.timings.clone();
        outcomes.push(o);
    }
    Ok(aggregate(&outcomes))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sillage_core::{Lang, Node, NodeKind, Span};

    use super::*;

    fn bag(ids: &[u64]) -> FeatureBag {
        FeatureBag::from_unsorted(ids.iter().copied())
    }

    #[test]
    fn difference_is_multiset() {
        let d = bag_difference(&bag(&[1, 1, 1, 2, 3]), &bag(&[1, 3, 4]));
        assert_eq!(d.entries(), &[(1, 2), (2, 1)]);
    }

    #[test]
    fn containment_and_precision_recall() {
        let fq = bag(&[1, 2, 3, 4]);
        let fr = bag(&[1, 2, 3, 5, 6, 7, 8]);
        assert_eq!(containment(&fq, &fr), 0.75);
        // Added = {5,6,7,8}; held = {5,6,9,10}: 2 hits.
        let (p, r) = added_precision_recall(&fq, &fr, &bag(&[5, 6, 9, 10]));
        assert_eq!((p, r), (0.5, 0.5));
        assert_eq!(added_precision_recall(&fq, &fq, &bag(&[5])), (0.0, 0.0));
        assert_eq!(containment(&FeatureBag::new(), &fr), 0.0);
    }

    #[test]
    fn extra_lines_counts_distinct_unmatched_lines() {
        let src: Arc<str> = Arc::from("a b\nc\nd e");
        let tok = |s: u32, t: &str| Node {
            kind: NodeKind::Token { text: t.into(), var: None },
            parent: Some(0),
            index_in_parent: 0,
            children: vec![],
            span: Span::new(s, s + 1),
            dropped_children: false,
        };
        let mut nodes = vec![Node {
            kind: NodeKind::Internal { label: "#####".into() },
            parent: None,
            index_in_parent: 0,
            children: vec![1, 2, 3, 4, 5],
            span: Span::new(0, 9),
            dropped_children: false,
        }];
        for (s, t) in [(0, "a"), (2, "b"), (4, "c"), (6, "d"), (8, "e")] {
            nodes.push(tok(s, t));
        }
        let tree = SimplifiedTree { lang: Lang::Java, source: src, nodes, leaves: vec![1, 2, 3, 4, 5] };
        assert_eq!(extra_lines(&tree, &[true, false, true, false, false]), 2);
        assert_eq!(extra_lines(&tree, &[true; 5]), 0);
        assert_eq!(extra_lines(&tree, &[]), 3);
    }

    #[test]
    fn aggregate_averages() {
        let outcomes = vec![
            QueryOutcome { contained: true, recommendations: 2, best: Some((1.0, 0.5)), extra_lines: vec![2, 4], ms: 10.0, timings: Default::default() },
            QueryOutcome { contained: false, recommendations: 0, best: None, extra_lines: vec![], ms: 30.0, timings: Default::default() },
            QueryOutcome { contained: true, recommendations: 1, best: Some((0.5, 0.25)), extra_lines: vec![3], ms: 20.0, timings: Default::default() },
        ];
        let r = aggregate(&outcomes);
        assert_eq!(r.sampled, 3);
        assert!((r.containment_rate - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(r.avg_precision, 0.75);
        assert_eq!(r.avg_recall, 0.375);
        assert_eq!(r.avg_recommendations, 1.0);
        assert_eq!(r.avg_extra_lines, 3.0);
        assert_eq!(r.p50_ms, 20.0);
        assert!((r.p95_ms - 29.0).abs() < 1e-9);
        let empty = aggregate(&[]);
        assert_eq!(empty.sampled, 0);
        assert_eq!(empty.containment_rate, 0.0);
    }
}
