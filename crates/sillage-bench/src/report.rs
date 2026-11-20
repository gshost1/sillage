//! Markdown and JSON formatters for benchmark reports.

use crate::{Ci, QualityReport, RecallMode, RecallReport};

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

fn ci_cell(ci: &Ci) -> String {
    format!("{} [{}, {}]", pct(ci.mean), pct(ci.low), pct(ci.high))
}

pub fn mode_name(mode: RecallMode) -> &'static str {
    match mode {
        RecallMode::Contiguous => "contiguous",
        RecallMode::NonContiguous => "non-contiguous",
    }
}

/// Table 4-style Markdown table: mean with 95% bootstrap CI per cell.
pub fn recall_markdown(r: &RecallReport) -> String {
    let mut out = format!(
        "### Recall ({}, {} queries)\n\n| System | Recall@1 (95% CI) | Recall@100 (95% CI) |\n|---|---|---|\n",
        mode_name(r.mode),
        r.sampled
    );
    for row in &r.rows {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            row.system,
            ci_cell(&row.recall_at_1),
            ci_cell(&row.recall_at_100)
        ));
    }
    out
}

pub fn recall_json(r: &RecallReport) -> String {
    serde_json::to_string_pretty(r).expect("recall report serializes")
}

pub fn quality_markdown(q: &QualityReport) -> String {
    format!(
        "### Recommendation quality (half-snippet queries, {} queries)\n\n\
         | Metric | Value |\n|---|---|\n\
         | Containment rate (≥ τ1) | {} |\n\
         | Avg precision of added features | {} |\n\
         | Avg recall of held-out features | {} |\n\
         | Avg recommendations / query | {:.2} |\n\
         | Avg extra lines / recommendation | {:.2} |\n\
         | Latency p50 | {:.1} ms |\n\
         | Latency p95 | {:.1} ms |\n\
         | Latency max | {:.1} ms |\n",
        q.sampled,
        pct(q.containment_rate),
        pct(q.avg_precision),
        pct(q.avg_recall),
        q.avg_recommendations,
        q.avg_extra_lines,
        q.p50_ms,
        q.p95_ms,
        q.max_ms
    ) + &phase_table(q)
}

fn phase_table(q: &QualityReport) -> String {
    if q.phases.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n| Phase | p50 | p95 | max |\n|---|---|---|---|\n");
    for p in &q.phases {
        s.push_str(&format!("| {} | {:.1} ms | {:.1} ms | {:.1} ms |\n", p.phase, p.p50_ms, p.p95_ms, p.max_ms));
    }
    s
}

pub fn quality_json(q: &QualityReport) -> String {
    serde_json::to_string_pretty(q).expect("quality report serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecallRow;

    fn ci(mean: f64, low: f64, high: f64) -> Ci {
        Ci { mean, low, high }
    }

    fn recall_report() -> RecallReport {
        RecallReport {
            mode: RecallMode::NonContiguous,
            sampled: 1000,
            rows: vec![
                RecallRow {
                    system: "BM25 Keywords".into(),
                    recall_at_1: ci(0.123, 0.101, 0.1449),
                    recall_at_100: ci(0.5, 0.47, 0.53),
                },
                RecallRow {
                    system: "Sillage".into(),
                    recall_at_1: ci(0.991, 0.985, 0.996),
                    recall_at_100: ci(1.0, 1.0, 1.0),
                },
            ],
        }
    }

    #[test]
    fn recall_markdown_table() {
        let md = recall_markdown(&recall_report());
        assert_eq!(
            md,
            "### Recall (non-contiguous, 1000 queries)\n\n\
             | System | Recall@1 (95% CI) | Recall@100 (95% CI) |\n|---|---|---|\n\
             | BM25 Keywords | 12.3% [10.1%, 14.5%] | 50.0% [47.0%, 53.0%] |\n\
             | Sillage | 99.1% [98.5%, 99.6%] | 100.0% [100.0%, 100.0%] |\n"
        );
    }

    #[test]
    fn recall_json_roundtrips() {
        let js = recall_json(&recall_report());
        let back: RecallReport = serde_json::from_str(&js).unwrap();
        assert_eq!(back.sampled, 1000);
        assert_eq!(back.mode, RecallMode::NonContiguous);
        assert_eq!(back.rows[1].system, "Sillage");
        let v: serde_json::Value = serde_json::from_str(&js).unwrap();
        assert_eq!(v["rows"][0]["recall_at_1"]["mean"], 0.123);
    }

    #[test]
    fn quality_formats() {
        let q = QualityReport {
            sampled: 200,
            containment_rate: 0.9,
            avg_precision: 0.4567,
            avg_recall: 0.25,
            avg_recommendations: 3.456,
            avg_extra_lines: 4.0,
            p50_ms: 12.34,
            p95_ms: 56.78,
            max_ms: 99.0,
            phases: vec![crate::PhaseLatency { phase: "phase1".into(), p50_ms: 1.0, p95_ms: 2.0, max_ms: 3.0 }],
        };
        let md = quality_markdown(&q);
        assert!(md.starts_with("### Recommendation quality (half-snippet queries, 200 queries)"));
        assert!(md.contains("| Containment rate (≥ τ1) | 90.0% |"));
        assert!(md.contains("| Avg precision of added features | 45.7% |"));
        assert!(md.contains("| Avg recommendations / query | 3.46 |"));
        assert!(md.contains("| Latency p95 | 56.8 ms |"));
        assert!(md.contains("| phase1 | 1.0 ms | 2.0 ms | 3.0 ms |"));
        let v: serde_json::Value = serde_json::from_str(&quality_json(&q)).unwrap();
        assert_eq!(v["sampled"], 200);
        assert_eq!(v["p50_ms"], 12.34);
    }
}
