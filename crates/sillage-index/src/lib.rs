//! Aroma featurization, Phase I inverted index, BM25 keyword baseline, corpus
//! construction and persistence.

mod bm25;
mod corpus;
mod featurize;
mod hash;
mod incremental;
mod index;
mod store;

pub use bm25::{Bm25, tokenize, tokenize_owned};
pub use corpus::{Corpus, CorpusStats, IndexParts, build_corpus, build_corpus_and_index, index_dir, index_files};
pub use incremental::{IncrementalIndex, Refresh, RefreshStats};
pub use featurize::{AromaFeaturizer, PaperFeaturizer, feature_debug, query_feature_debug};
pub use index::{COUNT_SAT, Hit, InvertedIndex, ResolvedQuery};
pub use store::{FORMAT_VERSION, from_bytes, load, save, to_bytes};
