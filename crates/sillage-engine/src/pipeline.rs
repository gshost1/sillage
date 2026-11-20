//! End-to-end Aroma pipeline: parse → Phase I → Phase II → Phase III.
//! This is the only module that touches `sillage_parse` and `sillage_index`.

use std::time::Instant;

use rayon::prelude::*;
use sillage_core::{FeatureBag, Lang, MethodId, MethodRecord, SimplifiedTree, lang_compatible};

use crate::filter::CompiledFilter;
use crate::{
    Engine, Output, QueryFilter, Ranked, Recommendation, SimilarMatch, Timings, cluster::cluster_scored,
    complete_calls, intersect, prune, rerank, rerank_top,
};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

impl Engine {
    /// The corpus record of method `id`.
    pub fn method(&self, id: MethodId) -> Option<&MethodRecord> {
        self.record(id)
    }

    fn record(&self, id: MethodId) -> Option<&MethodRecord> {
        match self.corpus.methods.get(id.0 as usize) {
            Some(r) if r.id == id => Some(r),
            _ => self.corpus.methods.iter().find(|r| r.id == id),
        }
    }

    /// `F(q)`: the query-mode features of a snippet, as the engine matches them.
    pub fn query_features(&self, lang: Lang, snippet: &str) -> anyhow::Result<FeatureBag> {
        let tree = sillage_parse::parse_snippet(lang, snippet)?;
        Ok(self.featurizer.featurize_query(&tree).total())
    }

    /// `(F(q), F_full(q))`: the query-mode features the engine matches, and the
    /// full featurization of the snippet (Phase II tie-breaks only).
    pub fn query_feature_pair(&self, lang: Lang, snippet: &str) -> anyhow::Result<(FeatureBag, FeatureBag)> {
        let tree = sillage_parse::parse_snippet(lang, snippet)?;
        Ok((self.featurizer.featurize_query(&tree).total(), self.featurizer.featurize(&tree).total()))
    }

    /// Whether method `id` may be retrieved for a `lang` query: its language is
    /// in the query's family ([`lang_compatible`]) and `filter` admits it.
    /// Applied to Phase I *before* the η1 cut, so in a mixed-language index
    /// other languages never take candidate slots and clusters never mix
    /// languages; likewise for filtered-out scopes, ids and paths.
    fn admissible(&self, lang: Lang, filter: Option<&CompiledFilter>, id: MethodId) -> bool {
        self.record(id).is_some_and(|r| lang_compatible(lang, r.lang) && filter.is_none_or(|f| f.admits(r)))
    }

    fn compile(&self, filter: &QueryFilter) -> Option<CompiledFilter> {
        (!filter.is_trivial()).then(|| filter.compile(&self.corpus, self.root.as_deref()))
    }

    /// Re-parses a Phase I candidate (methods that fail to parse are skipped).
    fn load_tree(&self, id: MethodId) -> Option<SimplifiedTree> {
        let rec = self.record(id)?;
        sillage_parse::parse_method(rec.lang, &rec.text, rec.body).ok()
    }

    /// `F(m)`: the full features of corpus method `id`'s body (`None` when it
    /// does not parse).
    pub fn method_features(&self, id: MethodId) -> Option<FeatureBag> {
        Some(self.featurizer.featurize(&self.load_tree(id)?).total())
    }

    /// Phases I and II, keeping the exact top `k` of the reranked list
    /// (see [`rerank_top`]); returns `(ranked, candidates parsed)`.
    fn ranked_top(
        &self,
        lang: Lang,
        fq: &FeatureBag,
        fq_full: &FeatureBag,
        k: usize,
        filter: Option<&CompiledFilter>,
    ) -> (Vec<Ranked>, usize) {
        let hits = self.index.search_bounded_where(fq, self.params.eta1, |id| self.admissible(lang, filter, id));
        let full = self.index.resolve(fq_full);
        // Pre-load tie-break bound: full_sim ≤ Σ_f min(F_full(q)_f, m_f).
        rerank_top(fq, fq_full, hits, k, None, self.featurizer.as_ref(), |id| self.load_tree(id), |id| full.bound(id))
    }

    /// Structural code search: Phases I and II, truncated to `n`.
    pub fn search(&self, lang: Lang, snippet: &str, n: usize) -> anyhow::Result<Vec<Ranked>> {
        let (fq, fq_full) = self.query_feature_pair(lang, snippet)?;
        Ok(self.search_features(lang, &fq, &fq_full, n))
    }

    /// [`search`](Self::search) for already computed query features
    /// ([`query_feature_pair`](Self::query_feature_pair)).
    pub fn search_features(&self, lang: Lang, fq: &FeatureBag, fq_full: &FeatureBag, n: usize) -> Vec<Ranked> {
        self.ranked_top(lang, fq, fq_full, n, None).0
    }

    /// [`search`](Self::search) over the methods `filter` admits (applied
    /// before the η1 cut).
    pub fn search_filtered(&self, lang: Lang, snippet: &str, n: usize, filter: &QueryFilter) -> anyhow::Result<Vec<Ranked>> {
        let (fq, fq_full) = self.query_feature_pair(lang, snippet)?;
        Ok(self.ranked_top(lang, &fq, &fq_full, n, self.compile(filter).as_ref()).0)
    }

    /// Structural similarity search for agents: the top `n` methods `filter`
    /// admits, ranked like [`search`](Self::search), with metadata, a
    /// `path:line` citation and the method source.
    pub fn find_similar(&self, lang: Lang, snippet: &str, n: usize, filter: &QueryFilter) -> anyhow::Result<Vec<SimilarMatch>> {
        Ok(self
            .search_filtered(lang, snippet, n, filter)?
            .into_iter()
            .filter_map(|r| {
                let m = self.record(r.id)?;
                Some(SimilarMatch {
                    id: r.id,
                    name: m.name.to_string(),
                    path: m.rel_path(),
                    line_start: m.line_start,
                    line_end: m.line_end(),
                    provenance: m.provenance(),
                    lang: m.lang,
                    is_test: m.is_test,
                    score: r.score,
                    sim: r.sim,
                    overlap: r.overlap,
                    snippet: m.text.to_string(),
                })
            })
            .collect())
    }

    /// Phases I and II over all η1 candidates, re-parsing every one of them
    /// (the unbounded reference for [`search`](Self::search)).
    pub fn search_exhaustive(&self, lang: Lang, snippet: &str, n: usize) -> anyhow::Result<Vec<Ranked>> {
        let (fq, fq_full) = self.query_feature_pair(lang, snippet)?;
        let cands: Vec<(MethodId, u32, SimplifiedTree)> = self
            .index
            .search_where(&fq, self.params.eta1, |id| self.admissible(lang, None, id))
            .into_par_iter()
            .filter_map(|(id, overlap)| Some((id, overlap, self.load_tree(id)?)))
            .collect();
        let mut ranked = rerank(&fq, &fq_full, cands, self.featurizer.as_ref());
        ranked.truncate(n);
        Ok(ranked)
    }

    /// Phase I only (feature-overlap baseline).
    pub fn features_only(&self, lang: Lang, snippet: &str, n: usize) -> anyhow::Result<Vec<MethodId>> {
        let fq = self.query_features(lang, snippet)?;
        Ok(self
            .index
            .search_where(&fq, n, |id| self.admissible(lang, None, id))
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

    /// Lexical BM25 baseline over methods in `lang`'s family.
    pub fn bm25_search(&self, lang: Lang, snippet: &str, n: usize) -> Vec<MethodId> {
        self.bm25
            .search_where(snippet, n, |id| self.admissible(lang, None, id))
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Full code recommendation (Phases I–III) with per-phase timings.
    pub fn recommend(&self, lang: Lang, snippet: &str) -> anyhow::Result<Output> {
        self.recommend_excluding(lang, snippet, &[])
    }

    /// [`recommend`](Self::recommend) with the methods in `exclude` removed from
    /// Phase I, before the η1 cut: they never reach N2, clustering, Jaccard
    /// dedup or the top-k (leave-one-out evaluation of a method's own snippet).
    pub fn recommend_excluding(&self, lang: Lang, snippet: &str, exclude: &[MethodId]) -> anyhow::Result<Output> {
        self.recommend_filtered(lang, snippet, &QueryFilter::new().exclude_ids(exclude.iter().copied()))
    }

    /// [`recommend`](Self::recommend) over the methods `filter` admits: scope,
    /// excluded ids and excluded paths are applied in Phase I before the η1
    /// cut, so they never take an N2, cluster, dedup or top-k slot.
    pub fn recommend_filtered(&self, lang: Lang, snippet: &str, filter: &QueryFilter) -> anyhow::Result<Output> {
        let featurizer = self.featurizer.as_ref();
        let start = Instant::now();
        let compiled = self.compile(filter);

        let t = Instant::now();
        let (fq, fq_full) = self.query_feature_pair(lang, snippet)?;
        let parse_ms = ms(t);

        // Only the top η2 of N2 above τ1 reach Phase III (`cluster` takes the
        // first η2 candidates with score > τ1, a prefix of the ranking), so N2
        // is computed only that far.
        let t = Instant::now();
        let hits = self.index.search_bounded_where(&fq, self.params.eta1, |id| self.admissible(lang, compiled.as_ref(), id));
        let phase1_ms = ms(t);

        let t = Instant::now();
        let full = self.index.resolve(&fq_full);
        let (n2, parsed) = rerank_top(
            &fq,
            &fq_full,
            hits,
            self.params.eta2,
            Some(self.params.tau1),
            featurizer,
            |id| self.load_tree(id),
            |id| full.bound(id),
        );
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
