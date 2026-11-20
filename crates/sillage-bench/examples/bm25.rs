//! BM25 baseline on a saved index's corpus, with the BM25 index rebuilt by the
//! current `sillage_index::tokenize` (for tokenizer ablations without a full
//! reindex). Reports BM25 Recall@1/@100.
//!
//! ```sh
//! cargo run --release -p sillage-bench --example bm25 -- data/java.sil 1000 contiguous 7
//! ```

use std::path::PathBuf;

use rayon::prelude::*;
use sillage_bench::recall::{build_queries, id_hits};
use sillage_bench::{RecallCfg, RecallMode};
use sillage_core::Params;
use sillage_engine::Engine;
use sillage_index::{AromaFeaturizer, Bm25};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let index = PathBuf::from(args.first().map(String::as_str).unwrap_or("data/java.sil"));
    let n: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(1000);
    let mode: RecallMode = args
        .get(2)
        .map(|s| s.parse().map_err(anyhow::Error::msg))
        .transpose()?
        .unwrap_or(RecallMode::Contiguous);
    let seed: u64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(7);
    let (corpus, inv, _) = sillage_index::load(&index)?;
    let bm25 = Bm25::build(&corpus);
    let terms = bm25.num_terms();
    let engine = Engine::new(corpus, inv, bm25, Box::new(AromaFeaturizer), Params::default());
    let queries = build_queries(&engine, &RecallCfg { n, mode, bootstrap: 0, seed });
    let hits: Vec<(bool, bool)> =
        queries.par_iter().map(|q| id_hits(q.orig, &engine.bm25_search(&q.snippet, 100))).collect();
    let r1 = hits.iter().filter(|h| h.0).count() as f64 / hits.len().max(1) as f64;
    let r100 = hits.iter().filter(|h| h.1).count() as f64 / hits.len().max(1) as f64;
    println!("{mode:?} n={} terms={terms} BM25 R@1 {:.1}% R@100 {:.1}%", hits.len(), 100.0 * r1, 100.0 * r100);
    Ok(())
}
