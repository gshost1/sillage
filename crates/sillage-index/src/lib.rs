//! Aroma featurization, Phase I inverted index, BM25 keyword baseline, corpus
//! construction and persistence.

mod bm25;
mod corpus;
mod featurize;
mod hash;
mod index;
mod store;

pub use bm25::{Bm25, tokenize};
pub use corpus::{Corpus, CorpusStats, build_corpus, build_corpus_and_index};
pub use featurize::{AromaFeaturizer, PaperFeaturizer, feature_debug, query_feature_debug};
pub use index::{COUNT_SAT, Hit, InvertedIndex, ResolvedQuery};
pub use store::{FORMAT_VERSION, from_bytes, load, save, to_bytes};
