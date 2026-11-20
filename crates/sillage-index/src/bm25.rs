//! Keyword-search baseline (paper §5.2.3, with Okapi BM25 in place of TF-IDF):
//! each method body is a bag of words, retrieved with `k1 = 1.2`, `b = 0.75`.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use sillage_core::MethodId;

use crate::corpus::Corpus;

const K1: f64 = 1.2;
const B: f64 = 0.75;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bm25 {
    /// Sorted unique terms.
    terms: Vec<Box<str>>,
    /// `post_docs[offsets[t]..offsets[t + 1]]` are dense doc indices containing `terms[t]`.
    offsets: Vec<u32>,
    post_docs: Vec<u32>,
    post_tf: Vec<u32>,
    /// Dense doc index → method ID.
    doc_ids: Vec<MethodId>,
    doc_len: Vec<u32>,
    avgdl: f64,
}

impl Bm25 {
    /// Indexes the body (`text[body]`) of every corpus method.
    pub fn build(corpus: &Corpus) -> Self {
        let docs: Vec<(MethodId, &str)> = corpus
            .methods
            .iter()
            .map(|m| {
                let text: &str = &m.text;
                let body = text.get(m.body.start as usize..m.body.end as usize).unwrap_or(text);
                (m.id, body)
            })
            .collect();
        Self::build_from_texts(&docs)
    }

    /// Indexes arbitrary `(MethodId, text)` documents.
    pub fn build_from_texts(docs: &[(MethodId, &str)]) -> Self {
        let mut doc_ids = Vec::with_capacity(docs.len());
        let mut doc_len = Vec::with_capacity(docs.len());
        // (term, doc, tf) triples, built per doc then sorted by term.
        let mut triples: Vec<(Box<str>, u32, u32)> = Vec::new();
        let mut tf: FxHashMap<String, u32> = FxHashMap::default();
        for (d, &(id, text)) in docs.iter().enumerate() {
            tf.clear();
            let mut len = 0u32;
            for w in tokenize(text) {
                *tf.entry(w).or_insert(0) += 1;
                len += 1;
            }
            doc_ids.push(id);
            doc_len.push(len);
            triples.extend(tf.drain().map(|(w, c)| (w.into_boxed_str(), d as u32, c)));
        }
        triples.sort_unstable();

        let mut terms: Vec<Box<str>> = Vec::new();
        let mut offsets = vec![0u32];
        let mut post_docs = Vec::with_capacity(triples.len());
        let mut post_tf = Vec::with_capacity(triples.len());
        for (w, d, c) in triples {
            if terms.last() != Some(&w) {
                if !terms.is_empty() {
                    offsets.push(post_docs.len() as u32);
                }
                terms.push(w);
            }
            post_docs.push(d);
            post_tf.push(c);
        }
        if !terms.is_empty() {
            offsets.push(post_docs.len() as u32);
        }

        let total: u64 = doc_len.iter().map(|&l| u64::from(l)).sum();
        let avgdl = if docs.is_empty() { 0.0 } else { total as f64 / docs.len() as f64 };
        Self { terms, offsets, post_docs, post_tf, doc_ids, doc_len, avgdl }
    }

    /// Top-`n` documents by BM25 score, sorted by `(score DESC, MethodId ASC)`.
    /// Only documents sharing at least one query term are returned.
    pub fn search(&self, query: &str, n: usize) -> Vec<(MethodId, f64)> {
        if n == 0 || self.doc_ids.is_empty() {
            return Vec::new();
        }
        let mut q: Vec<String> = tokenize(query).collect();
        q.sort_unstable();
        q.dedup();

        let num_docs = self.doc_ids.len() as f64;
        let avgdl = if self.avgdl > 0.0 { self.avgdl } else { 1.0 };
        let mut scores = vec![0.0f64; self.doc_ids.len()];
        let mut touched = vec![false; self.doc_ids.len()];
        for w in &q {
            let Ok(t) = self.terms.binary_search_by(|x| x.as_ref().cmp(w.as_str())) else {
                continue;
            };
            let (lo, hi) = (self.offsets[t] as usize, self.offsets[t + 1] as usize);
            let df = (hi - lo) as f64;
            let idf = (1.0 + (num_docs - df + 0.5) / (df + 0.5)).ln();
            for p in lo..hi {
                let d = self.post_docs[p] as usize;
                let tf = f64::from(self.post_tf[p]);
                let norm = K1 * (1.0 - B + B * f64::from(self.doc_len[d]) / avgdl);
                scores[d] += idf * tf * (K1 + 1.0) / (tf + norm);
                touched[d] = true;
            }
        }

        let mut hits: Vec<(MethodId, f64)> = touched
            .iter()
            .enumerate()
            .filter(|&(_, &t)| t)
            .map(|(d, _)| (self.doc_ids[d], scores[d]))
            .collect();
        let cmp = |a: &(MethodId, f64), b: &(MethodId, f64)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
        if hits.len() > n {
            hits.select_nth_unstable_by(n, cmp);
            hits.truncate(n);
        }
        hits.sort_unstable_by(cmp);
        hits
    }

    pub fn num_terms(&self) -> usize {
        self.terms.len()
    }
}

/// Identifier and number words, case-sensitive and whole (`getChildCount`,
/// `HTTPServer_v2`). Used identically for documents and queries.
///
/// Stage 3 ablation on the Java benchmark (BM25 Recall@1, contiguous /
/// non-contiguous, n = 1000, seed 7): lower-cased words plus camelCase /
/// snake_case sub-words 73.2% / 81.0%; lower-cased whole words 73.3% / 81.0%;
/// sub-words only 68.9% / 79.5%; case-sensitive whole words **74.0% / 81.9%**.
/// Sub-words dilute IDF with common fragments (`get`, `set`, `test`).
pub fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_keeps_whole_case_sensitive_identifiers() {
        let words: Vec<String> = tokenize("x.getChildCount() + HTTPServer_v2; $el = 42").collect();
        assert_eq!(words, ["x", "getChildCount", "HTTPServer_v2", "$el", "42"]);
    }
}
