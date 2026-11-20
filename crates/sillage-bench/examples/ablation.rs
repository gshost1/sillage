//! Recall ablation and miss diagnostics on a corpus directory.
//!
//! ```sh
//! cargo run --release -p sillage-bench --example ablation -- \
//!     data/corpus java 500 contiguous 7 [misses]
//! ```
//!
//! Builds the corpus once, then indexes it with the default `AromaFeaturizer`
//! (statement-sequence normalisation) and the literal `PaperFeaturizer`, and runs
//! the §5 recall benchmark on each. With `misses`, prints every Sillage Recall@1
//! miss of the default featurizer: query, original rank, and the top result.

use std::path::PathBuf;
use std::time::Instant;

use rayon::prelude::*;
use sillage_bench::recall::{build_queries, evaluate_query};
use sillage_bench::{RecallCfg, RecallMode, bootstrap::bootstrap_hits};
use sillage_core::{Featurizer, Lang, Params};
use sillage_engine::Engine;
use sillage_index::{AromaFeaturizer, Bm25, InvertedIndex, PaperFeaturizer};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let corpus_dir = PathBuf::from(args.first().map(String::as_str).unwrap_or("data/corpus"));
    let lang: Lang = args.get(1).map(String::as_str).unwrap_or("java").parse()?;
    let n: usize = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(500);
    let mode: RecallMode = args
        .get(3)
        .map(|s| s.parse().map_err(anyhow::Error::msg))
        .transpose()?
        .unwrap_or(RecallMode::Contiguous);
    let seed: u64 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(7);
    let show_misses = args.get(5).is_some_and(|s| s == "misses");

    let t = Instant::now();
    let corpus = sillage_index::build_corpus(&corpus_dir, &[lang]);
    eprintln!("corpus: {} methods in {:.1}s", corpus.methods.len(), t.elapsed().as_secs_f64());

    let variants: Vec<(&str, Box<dyn Featurizer>)> =
        vec![("aroma+seqnorm (default)", Box::new(AromaFeaturizer)), ("paper-literal", Box::new(PaperFeaturizer))];
    println!("| Featurizer | Mode | n | BM25 R@1 | BM25 R@100 | Features R@1 | Features R@100 | Sillage R@1 | Sillage R@100 |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for (k, (name, fz)) in variants.into_iter().enumerate() {
        let t = Instant::now();
        let inv = InvertedIndex::build(&corpus, fz.as_ref());
        let bm25 = Bm25::build(&corpus);
        eprintln!("{name}: indexed in {:.1}s", t.elapsed().as_secs_f64());
        let engine = Engine::new(corpus.clone(), inv, bm25, fz, Params::default());
        let cfg = RecallCfg { n, mode, bootstrap: 1000, seed };
        let queries = build_queries(&engine, &cfg);
        let t = Instant::now();
        let per: Vec<[(bool, bool); 3]> = queries.par_iter().map(|q| evaluate_query(&engine, q)).collect();
        eprintln!("{name}: {} queries in {:.1}s", queries.len(), t.elapsed().as_secs_f64());
        let pct = |sys: usize, at1: bool| {
            let h: Vec<bool> = per.iter().map(|r| if at1 { r[sys].0 } else { r[sys].1 }).collect();
            format!("{:.1}%", 100.0 * bootstrap_hits(&h, 1000, seed).mean)
        };
        println!(
            "| {name} | {mode:?} | {} | {} | {} | {} | {} | {} | {} |",
            queries.len(),
            pct(0, true),
            pct(0, false),
            pct(1, true),
            pct(1, false),
            pct(2, true),
            pct(2, false)
        );
        if show_misses && k == 0 {
            for (q, r) in queries.iter().zip(&per) {
                if r[2].0 {
                    continue;
                }
                let ranked = engine.search(q.lang, &q.snippet, 100)?;
                let pos = ranked.iter().position(|x| x.id == q.orig);
                let orig = &engine.corpus.methods[q.orig.0 as usize];
                let top = ranked.first().map(|x| (&engine.corpus.methods[x.id.0 as usize], x.sim));
                eprintln!(
                    "\n--- MISS {}:{}:{} {} | orig rank {:?} sim {:?} | top {:?}",
                    orig.repo,
                    orig.path,
                    orig.line_start,
                    orig.name,
                    pos.map(|p| p + 1),
                    pos.map(|p| ranked[p].sim),
                    top.map(|(m, s)| format!("{}:{}:{} {} sim {s}", m.repo, m.path, m.line_start, m.name)),
                );
                eprintln!("{}", q.snippet);
            }
        }
    }
    Ok(())
}
