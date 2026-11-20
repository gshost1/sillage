//! Single-threaded breakdown of Phase I/II cost per query.
//!
//! ```sh
//! cargo run --release -p sillage-bench --example profile -- data/java.sil 30 7 [contiguous|noncontiguous]
//! ```
//!
//! For `n` §5 queries, times (summed over the η1 candidates of each query, on one
//! thread) the index search, raw tree-sitter parse, `parse_method`, `featurize`,
//! `prune` and `retain`, and reports how many candidates the multiset upper bound
//! `Σ min(q_f, m_f)` cannot rule out of the top η2.

use std::path::PathBuf;
use std::time::Instant;

use sillage_bench::recall::build_queries;
use sillage_bench::{RecallCfg, RecallMode};
use sillage_core::{Featurizer, Params};
use sillage_engine::{Engine, prune};
use sillage_index::AromaFeaturizer;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let index = PathBuf::from(args.first().map(String::as_str).unwrap_or("data/java.sil"));
    let n: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(30);
    let seed: u64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(7);
    let mode: RecallMode = args
        .get(3)
        .map(|s| s.parse().map_err(anyhow::Error::msg))
        .transpose()?
        .unwrap_or(RecallMode::Contiguous);

    let t = Instant::now();
    let (corpus, inv, bm25) = sillage_index::load(&index)?;
    eprintln!("load: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let engine = Engine::new(corpus, inv, bm25, Box::new(AromaFeaturizer), Params::default());
    let fz = AromaFeaturizer;
    let queries = build_queries(&engine, &RecallCfg { n, mode, bootstrap: 0, seed });
    let lang = queries.first().map(|q| q.lang).unwrap_or(sillage_core::Lang::Java);
    let mut ts = tree_sitter::Parser::new();
    ts.set_language(&sillage_parse::ts_language(lang))?;

    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let (mut s_search, mut s_ts, mut s_parse, mut s_feat, mut s_prune, mut s_retain) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut nodes, mut cands) = (0usize, 0usize);
    let mut needed: Vec<usize> = Vec::new();
    for q in &queries {
        let fq = fz.featurize(&sillage_parse::parse_snippet(q.lang, &q.snippet)?).total();
        let t = Instant::now();
        let hits = engine.index.search(&fq, engine.params.eta1);
        s_search += ms(t);
        let mut sims: Vec<(u64, u64)> = Vec::new();
        for (id, _) in hits {
            let rec = &engine.corpus.methods[id.0 as usize];
            let t = Instant::now();
            let wrapped = format!("class __S {{\n{}\n}}", rec.text);
            let _ = ts.parse(&wrapped, None);
            s_ts += ms(t);
            let t = Instant::now();
            let Ok(tree) = sillage_parse::parse_method(rec.lang, &rec.text, rec.body) else { continue };
            s_parse += ms(t);
            nodes += tree.nodes.len();
            cands += 1;
            let t = Instant::now();
            let lf = fz.featurize(&tree);
            s_feat += ms(t);
            let t = Instant::now();
            let p = prune(&fq, &tree, &lf);
            s_prune += ms(t);
            sims.push((p.sim, fq.intersect_count(&lf.total())));
            let t = Instant::now();
            let _ = tree.retain(&p.keep);
            s_retain += ms(t);
        }
        let mut by_sim: Vec<u64> = sims.iter().map(|x| x.0).collect();
        by_sim.sort_unstable_by(|a, b| b.cmp(a));
        let kth = by_sim.get(engine.params.eta2 - 1).copied().unwrap_or(0);
        needed.push(sims.iter().filter(|x| x.1 >= kth).count());
    }
    needed.sort_unstable();
    let k = queries.len().max(1) as f64;
    println!(
        "queries {} | candidates/query {:.0} | nodes/candidate {:.0}",
        queries.len(),
        cands as f64 / k,
        nodes as f64 / cands.max(1) as f64
    );
    println!(
        "per query, single thread (ms): search {:.1} | tree-sitter alone {:.1} | parse_method {:.1} | featurize {:.1} | prune (incl. retain) {:.1} | retain {:.1}",
        s_search / k,
        s_ts / k,
        s_parse / k,
        s_feat / k,
        s_prune / k,
        s_retain / k
    );
    if !needed.is_empty() {
        println!(
            "candidates with ub >= η2-th sim: p50 {} p90 {} max {}",
            needed[needed.len() / 2],
            needed[needed.len() * 9 / 10],
            needed[needed.len() - 1]
        );
    }
    Ok(())
}
