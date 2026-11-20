//! End-to-end Aroma pipeline: parse → Phase I → Phase II → Phase III.
//! This is the only module that touches `sillage_parse` and `sillage_index`.

use std::time::Instant;

use rayon::prelude::*;
use sillage_core::{FeatureBag, Lang, MethodId, MethodRecord, SimplifiedTree};

use crate::{
    Engine, Output, Ranked, Recommendation, Timings, cluster::cluster_scored, complete_calls, intersect, prune,
    rerank, rerank_top,
};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

impl Engine {
    fn record(&self, id: MethodId) -> Option<&MethodRecord> {
        match self.corpus.methods.get(id.0 as usize) {
            Some(r) if r.id == id => Some(r),
            _ => self.corpus.methods.iter().find(|r| r.id == id),
        }
    }

    fn query_features(&self, lang: Lang, snippet: &str) -> anyhow::Result<FeatureBag> {
        let tree = sillage_parse::parse_snippet(lang, snippet)?;
        Ok(self.featurizer.featurize_query(&tree).total())
    }

    /// Re-parses a Phase I candidate (methods that fail to parse are skipped).
    fn load_tree(&self, id: MethodId) -> Option<SimplifiedTree> {
        let rec = self.record(id)?;
        sillage_parse::parse_method(rec.lang, &rec.text, rec.body).ok()
    }

    /// Phases I and II, keeping the exact top `k` of the reranked list
    /// (see [`rerank_top`]); returns `(ranked, candidates parsed)`.
    fn ranked_top(&self, fq: &FeatureBag, k: usize) -> (Vec<Ranked>, usize) {
        let hits = self.index.search_bounded(fq, self.params.eta1);
        rerank_top(fq, hits, k, None, self.featurizer.as_ref(), |id| self.load_tree(id))
    }

    /// Structural code search: Phases I and II, truncated to `n`.
    pub fn search(&self, lang: Lang, snippet: &str, n: usize) -> anyhow::Result<Vec<Ranked>> {
        let fq = self.query_features(lang, snippet)?;
        Ok(self.ranked_top(&fq, n).0)
    }

    /// Phases I and II over all η1 candidates, re-parsing every one of them
    /// (the unbounded reference for [`search`](Self::search)).
    pub fn search_exhaustive(&self, lang: Lang, snippet: &str, n: usize) -> anyhow::Result<Vec<Ranked>> {
        let fq = self.query_features(lang, snippet)?;
        let cands: Vec<(MethodId, u32, SimplifiedTree)> = self
            .index
            .search(&fq, self.params.eta1)
            .into_par_iter()
            .filter_map(|(id, overlap)| Some((id, overlap, self.load_tree(id)?)))
            .collect();
        let mut ranked = rerank(&fq, cands, self.featurizer.as_ref());
        ranked.truncate(n);
        Ok(ranked)
    }

    /// Phase I only (feature-overlap baseline).
    pub fn features_only(&self, lang: Lang, snippet: &str, n: usize) -> anyhow::Result<Vec<MethodId>> {
        let fq = self.query_features(lang, snippet)?;
        Ok(self.index.search(&fq, n).into_iter().map(|(id, _)| id).collect())
    }

    /// Lexical BM25 baseline.
    pub fn bm25_search(&self, snippet: &str, n: usize) -> Vec<MethodId> {
        self.bm25.search(snippet, n).into_iter().map(|(id, _)| id).collect()
    }

    /// Full code recommendation (Phases I–III) with per-phase timings.
    pub fn recommend(&self, lang: Lang, snippet: &str) -> anyhow::Result<Output> {
        let featurizer = self.featurizer.as_ref();
        let start = Instant::now();

        let t = Instant::now();
        let fq = self.query_features(lang, snippet)?;
        let parse_ms = ms(t);

        // Only the top η2 of N2 above τ1 reach Phase III (`cluster` takes the
        // first η2 candidates with score > τ1, a prefix of the ranking), so N2
        // is computed only that far.
        let t = Instant::now();
        let hits = self.index.search_bounded(&fq, self.params.eta1);
        let phase1_ms = ms(t);

        let t = Instant::now();
        let (n2, parsed) = rerank_top(&fq, hits, self.params.eta2, Some(self.params.tau1), featurizer, |id| {
            self.load_tree(id)
        });
        let phase2_ms = ms(t);

        let t = Instant::now();
        let mut clusters = cluster_scored(&fq, &n2, &self.params, featurizer);
        clusters.truncate(self.params.k);
        let recommendations: Vec<Recommendation> = clusters
            .into_par_iter()
            .map(|c| {
                let mut tree = intersect(&c.tuple, &fq, &n2, &self.params, featurizer);
                if self.params.complete_calls && c.tuple.len() > 1 {
                    tree = complete_calls(&tree, &n2[c.tuple[0]].tree);
                }
                let query_match = prune(&fq, &tree, &featurizer.featurize(&tree)).keep;
                Recommendation {
                    cluster: c.tuple.iter().map(|&i| n2[i].id).collect(),
                    tree,
                    query_match,
                    l: c.l,
                    s: c.s,
                }
            })
            .collect();
        let phase3_ms = ms(t);

        Ok(Output {
            n2,
            recommendations,
            timings: Timings { parse_ms, phase1_ms, phase2_ms, phase3_ms, total_ms: ms(start), parsed },
        })
    }
}
