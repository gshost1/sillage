//! §5 recall (Table 4) and §4.2 quality benchmarks.

use serde::{Deserialize, Serialize};
use sillage_engine::Engine;

pub mod bootstrap;
pub mod quality;
pub mod recall;
pub mod report;
pub mod sample;

pub use bootstrap::bootstrap_ci;
pub use report::{quality_json, quality_markdown, recall_json, recall_markdown};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecallMode {
    Contiguous,
    NonContiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallCfg {
    pub n: usize,
    pub mode: RecallMode,
    pub bootstrap: usize,
    pub seed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ci {
    pub mean: f64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRow {
    pub system: String,
    /// §5 Recall@1 (Decision 9): strict, or the rank-1 method is a feature
    /// duplicate of the original (every system), or the original ties rank 1
    /// at the full query similarity `|F(q)|` (Sillage only).
    pub recall_at_1: Ci,
    pub recall_at_100: Ci,
    /// Strict Recall@1: the original method is ranked first.
    #[serde(default)]
    pub strict_recall_at_1: Ci,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallReport {
    pub mode: RecallMode,
    pub sampled: usize,
    pub rows: Vec<RecallRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCfg {
    pub n: usize,
    pub seed: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityReport {
    pub sampled: usize,
    pub containment_rate: f64,
    pub avg_precision: f64,
    pub avg_recall: f64,
    pub avg_recommendations: f64,
    pub avg_extra_lines: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    /// Slowest query end to end.
    #[serde(default)]
    pub max_ms: f64,
    /// Per-phase latency percentiles (`parse`, `phase1`, `phase2`, `phase3`).
    #[serde(default)]
    pub phases: Vec<PhaseLatency>,
    /// Kept recommendations by cluster size.
    #[serde(default)]
    pub cluster_sizes: ClusterSizes,
    /// Share of queries with at least one kept multi-method cluster.
    #[serde(default)]
    pub multi_cluster_rate: f64,
    /// Share of queries with a recommendation whose every added (rendered,
    /// non-placeholder) line occurs verbatim, whitespace-normalized, in the
    /// held-out half: an automated proxy for §4.2's "exact recommendation".
    #[serde(default)]
    pub exact_match_rate: f64,
    /// Best recommendation per query (by line F1): share of its added lines that
    /// occur in the held-out half, averaged over queries with recommendations.
    #[serde(default)]
    pub avg_line_precision: f64,
    /// Same recommendation: share of held-out lines it reproduces.
    #[serde(default)]
    pub avg_line_recall: f64,
}

/// Histogram of cluster sizes `|cluster|` over kept recommendations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterSizes {
    pub singleton: usize,
    pub two: usize,
    pub three_to_five: usize,
    pub six_plus: usize,
}

impl ClusterSizes {
    pub fn add(&mut self, size: usize) {
        match size {
            0 | 1 => self.singleton += 1,
            2 => self.two += 1,
            3..=5 => self.three_to_five += 1,
            _ => self.six_plus += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.singleton + self.two + self.three_to_five + self.six_plus
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseLatency {
    pub phase: String,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl std::str::FromStr for RecallMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "contiguous" => Ok(RecallMode::Contiguous),
            "noncontiguous" => Ok(RecallMode::NonContiguous),
            other => Err(format!("unknown recall mode: {other}")),
        }
    }
}

/// Paper §5 / Table 4: Recall@1 and Recall@100 of BM25, feature search and Sillage.
pub fn recall(engine: &Engine, cfg: RecallCfg) -> anyhow::Result<RecallReport> {
    recall::run(engine, cfg)
}

/// Paper §4.2: recommendation quality on half-snippet queries.
pub fn quality(engine: &Engine, cfg: QualityCfg) -> anyhow::Result<QualityReport> {
    quality::run(engine, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_mode_parses() {
        assert_eq!("contiguous".parse::<RecallMode>().unwrap(), RecallMode::Contiguous);
        assert_eq!("non-contiguous".parse::<RecallMode>().unwrap(), RecallMode::NonContiguous);
        assert_eq!("noncontiguous".parse::<RecallMode>().unwrap(), RecallMode::NonContiguous);
        assert!("random".parse::<RecallMode>().is_err());
    }
}
