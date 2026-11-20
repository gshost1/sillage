use serde::{Deserialize, Serialize};
use sillage_engine::Engine;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ci {
    pub mean: f64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRow {
    pub system: String,
    pub recall_at_1: Ci,
    pub recall_at_100: Ci,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub sampled: usize,
    pub containment_rate: f64,
    pub avg_precision: f64,
    pub avg_recall: f64,
    pub avg_recommendations: f64,
    pub avg_extra_lines: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

pub fn recall(_engine: &Engine, _cfg: RecallCfg) -> anyhow::Result<RecallReport> {
    todo!("Slice D")
}

pub fn quality(_engine: &Engine, _cfg: QualityCfg) -> anyhow::Result<QualityReport> {
    todo!("Slice D")
}
