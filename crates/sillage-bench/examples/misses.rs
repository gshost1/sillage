//! Sillage Recall@1 misses of the §5 benchmark on a saved index.
//!
//! ```sh
//! cargo run --release -p sillage-bench --example misses -- data/java.sil 1000 noncontiguous 7 [limit]
//! ```
//!
//! Prints a summary (miss count, original-rank histogram, similarity gap) and,
//! for up to `limit` misses (default 40), the query, the original's rank and
//! pruned similarity, and the top result. With `SILLAGE_DIFF=1`, each miss also
//! lists the query features the original method lacks (`q - orig`, readable
//! Table 2 notation) and the best possible similarity `|F(q) ∩ F(orig)|`
//! (when it exceeds the pruned similarity, greedy prune lost features).

use std::collections::BTreeMap;
use std::path::PathBuf;

use rayon::prelude::*;
use sillage_bench::recall::{build_queries, ranked_hits};
use sillage_bench::{RecallCfg, RecallMode};
use sillage_core::{Featurizer, MethodId, Params};
use sillage_engine::Engine;
use sillage_core::SimplifiedTree;
use sillage_index::{AromaFeaturizer, feature_debug, query_feature_debug};

/// Multiset of readable features of every leaf.
fn readable(tree: &SimplifiedTree, query: bool) -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    for i in 0..tree.leaves.len() {
        let fs = if query { query_feature_debug(tree, i) } else { feature_debug(tree, i) };
        for f in fs {
            *m.entry(f).or_insert(0) += 1;
        }
    }
    m
}

/// `a - b` as a multiset, rendered one feature per entry.
fn minus(a: &BTreeMap<String, i64>, b: &BTreeMap<String, i64>) -> Vec<String> {
    a.iter()
        .filter_map(|(k, &c)| {
            let d = c - b.get(k).copied().unwrap_or(0);
            (d > 0).then(|| if d > 1 { format!("{k} x{d}") } else { k.clone() })
        })
        .collect()
}

struct Hit {
    id: MethodId,
    sim: u64,
    overlap: u32,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let index = PathBuf::from(args.first().map(String::as_str).unwrap_or("data/java.sil"));
    let n: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(1000);
    let mode: RecallMode = args
        .get(2)
        .map(|s| s.parse().map_err(anyhow::Error::msg))
        .transpose()?
        .unwrap_or(RecallMode::NonContiguous);
    let seed: u64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(7);
    let limit: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(40);
    let diff = std::env::var_os("SILLAGE_DIFF").is_some();

    let (corpus, inv, bm25) = sillage_index::load(&index)?;
    let engine = Engine::new(corpus, inv, bm25, Box::new(AromaFeaturizer), Params::default());
    let queries = build_queries(&engine, &RecallCfg { n, mode, bootstrap: 0, seed });
    let results: Vec<_> = queries
        .par_iter()
        .map(|q| {
            // Keep only (id, sim, overlap): full `Ranked` values hold two trees
            // each, and 1000 queries x 100 of them need several GB.
            let ranked: Vec<Hit> = engine
                .search(q.lang, &q.snippet, 100)
                .unwrap_or_default()
                .into_iter()
                .map(|r| Hit { id: r.id, sim: r.sim, overlap: r.overlap })
                .collect();
            let pairs: Vec<(MethodId, u64)> = ranked.iter().map(|r| (r.id, r.sim)).collect();
            (ranked_hits(q.orig, &pairs), ranked)
        })
        .collect();
    let misses: Vec<usize> = (0..queries.len()).filter(|&i| !results[i].0.0).collect();
    let mut hist = [0usize; 5]; // rank 2, 3-5, 6-10, 11-100, absent
    for &i in &misses {
        let pos = results[i].1.iter().position(|r| r.id == queries[i].orig);
        hist[match pos {
            Some(1) => 0,
            Some(2..=4) => 1,
            Some(5..=9) => 2,
            Some(_) => 3,
            None => 4,
        }] += 1;
    }
    // Fingerprint of the sampled query set, to check that a change keeps the
    // benchmark's queries byte-identical (apples-to-apples).
    let fingerprint = {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        for q in &queries {
            (q.orig.0, &q.snippet).hash(&mut h);
        }
        h.finish()
    };
    println!("query set fingerprint {fingerprint:016x}");
    if let Some(path) = std::env::var_os("SILLAGE_DUMP") {
        let all: String = queries.iter().map(|q| format!("=== {}\n{}\n", q.orig.0, q.snippet)).collect();
        std::fs::write(path, all)?;
    }
    println!(
        "{} queries, {} R@1 misses ({:.1}%) | orig rank 2: {} | 3-5: {} | 6-10: {} | 11-100: {} | absent: {}",
        queries.len(),
        misses.len(),
        100.0 * (queries.len() - misses.len()) as f64 / queries.len().max(1) as f64,
        hist[0],
        hist[1],
        hist[2],
        hist[3],
        hist[4]
    );
    for &i in misses.iter().take(limit) {
        let q = &queries[i];
        let ranked = &results[i].1;
        let fq = AromaFeaturizer.featurize_query(&sillage_parse::parse_snippet(q.lang, &q.snippet)?).total();
        let pos = ranked.iter().position(|r| r.id == q.orig);
        let orig = &engine.corpus.methods[q.orig.0 as usize];
        let Some(top) = ranked.first() else {
            println!("\n--- MISS {}:{}:{} {} | no results", orig.repo, orig.path, orig.line_start, orig.name);
            println!("{}", q.snippet);
            continue;
        };
        let tm = &engine.corpus.methods[top.id.0 as usize];
        println!(
            "\n--- MISS {}:{}:{} {} | |F(q)| {} | orig rank {:?} sim {:?} overlap {:?} | top {}:{}:{} {} sim {} overlap {}",
            orig.repo,
            orig.path,
            orig.line_start,
            orig.name,
            fq.total(),
            pos.map(|p| p + 1),
            pos.map(|p| ranked[p].sim),
            pos.map(|p| ranked[p].overlap),
            tm.repo,
            tm.path,
            tm.line_start,
            tm.name,
            top.sim,
            top.overlap
        );
        println!("{}", q.snippet);
        if diff {
            let qt = sillage_parse::parse_snippet(q.lang, &q.snippet)?;
            let load = |id: MethodId| {
                let r = &engine.corpus.methods[id.0 as usize];
                sillage_parse::parse_method(r.lang, &r.text, r.body)
            };
            let (ot, tt) = (load(q.orig)?, load(top.id)?);
            let ideal = fq.intersect_count(&AromaFeaturizer.featurize(&ot).total());
            let rq = readable(&qt, true);
            let (ro, rt) = (readable(&ot, false), readable(&tt, false));
            let missing = minus(&rq, &ro);
            println!("  ideal |F(q) ∩ F(orig)| = {ideal}");
            println!("  q - orig ({}): {}", missing.len(), missing.join(" | "));
            let gained: Vec<&String> =
                missing.iter().filter(|f| rt.contains_key(f.split(" x").next().unwrap_or(f))).collect();
            println!("  ... of which top has: {gained:?}");
        }
    }
    Ok(())
}
