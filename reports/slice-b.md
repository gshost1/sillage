# Slice B — `crates/sillage-index`

## Status: compiles with zero warnings; all 26 tests pass

The first attempt could not build because `/tmp` hit its disk quota (EDQUOT). The rerun on
2026-11-20 used a target dir on NVMe, and no code changes were needed:

```sh
mkdir -p /home/george/Projects/sillage/target-slice-b/tmp
cd /home/george/Projects/sillage && export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/home/george/Projects/sillage/target-slice-b" TMPDIR="/home/george/Projects/sillage/target-slice-b/tmp"
cargo test -p sillage-index
```

Exit code 0, with 0 warnings after a forced rebuild of the crate (`touch src/lib.rs`).

| Binary | Passed |
|---|---|
| unit tests (`src/lib.rs`) | 6 |
| `tests/bm25_store.rs` | 7 |
| `tests/index_oracle.rs` (proptest plus a 2000-method seeded oracle) | 6 |
| `tests/paper_features.rs` | 7 |
| doc-tests | 0 |

```
running 6 tests
test bm25::tests::tokenize_splits_identifiers ... ok
test corpus::tests::method_hash_ignores_whitespace_and_name ... ok
test corpus::tests::split_repo_paths ... ok
test hash::tests::encoder_is_deterministic_and_resets ... ok
test hash::tests::length_prefix_prevents_field_aliasing ... ok
test hash::tests::tags_separate_families ... ok
test result: ok. 6 passed; 0 failed

running 7 tests  (tests/bm25_store.rs)
test bm25_prefers_rarer_terms_and_shorter_docs ... ok
test bm25_ties_break_by_method_id ... ok
test bm25_indexes_body_not_signature ... ok
test bm25_empty_cases ... ok
test bm25_ranks_relevant_methods ... ok
test load_rejects_bad_files ... ok
test save_load_roundtrip ... ok
test result: ok. 7 passed; 0 failed

running 6 tests  (tests/index_oracle.rs)
test duplicate_ids_merge_and_num_methods_grows ... ok
test set_semantics_ignore_counts ... ok
test edge_cases ... ok
test aroma_query_ranks_itself_first ... ok
test search_matches_oracle ... ok
test search_matches_oracle_large_seeded ... ok
test result: ok. 6 passed; 0 failed; finished in 0.30s

running 7 tests  (tests/paper_features.rs)
test member_context_falls_back_to_slot ... ok
test member_context_for_receiver ... ok
test out_of_range_leaf_is_empty ... ok
test table2_view30 ... ok
test first_and_last_leaves_have_one_sibling ... ok
test table2_zero20 ... ok
test featurize_matches_debug_and_is_alpha_invariant ... ok
test result: ok. 7 passed; 0 failed

Doc-tests sillage_index: 0 tests
```

Nothing was committed, because the other slices may still be writing their crates.

## Modules

| File | Contents |
|---|---|
| `src/lib.rs` | Re-exports; every public path from the stub is kept (`AromaFeaturizer`, `feature_debug`, `Corpus`, `build_corpus`, `InvertedIndex`, `Bm25`, `save`, `load`). Also adds `InvertedIndex::build_from_trees`, `Bm25::build_from_texts`, `Bm25::num_terms`, `tokenize` and `FORMAT_VERSION`. |
| `src/hash.rs` | Canonical encoding: a family tag byte (1 token, 2 parent, 3 sibling, 4 var-usage), then type-tagged fields (`S`+u32 len+bytes, `N`+u16), with ctx tags `c`/`m`. Hashed with `xxh3_64`. |
| `src/featurize.rs` | The four §3.2 families. Parent features use `ancestors().take(3)` with no depth in the key. Var-usage contexts are `Slot(i, L(t))`, or `Member(first non-local token of N(t))` when `L(t) == "#.#"`, falling back to `Slot`. PrevUse/NextUse are precomputed in O(n) per tree, and member lookups are cached per parent. |
| `src/index.rs` | CSR index (`feature_ids`, `offsets`, `postings`, `num_methods`). The build parses and featurizes with rayon, takes `S(F(m))`, builds a dense feature dictionary, then counts, prefix-sums and fills (about 12 B per posting at peak, not 16). Duplicate method IDs get their sets unioned. `num_methods` grows to cover the largest ID. Search counts overlap in a dense `Vec<u16>` (saturating), then uses `select_nth_unstable` and a sort on `(overlap DESC, id ASC)`. |
| `src/bm25.rs` | Okapi BM25 (k1 1.2, b 0.75, Lucene idf `ln(1 + (N-df+0.5)/(df+0.5))`) over CSR postings of the method **body** (`text[body]`, matching §5.2.3 "method body as a bag of words"). The tokenizer lowercases identifier/number words and adds camelCase/snake_case sub-words. Repeated query terms are deduplicated. |
| `src/corpus.rs` | Walks with `ignore::WalkBuilder`, filters by `lang_from_path ∈ langs`, and skips non-UTF-8 files and files over 2 MiB. Each top-level subdir is a repo; files directly under root go to repo `"."`. Dedup runs in this order: project (hash of the sorted `(path, file hash)` list), file (content hash), method (whitespace-stripped body hash plus lang). The first occurrence is kept, in `(repo, path, line_start, name)` order, and IDs are assigned `MethodId(0..)`. |
| `src/store.rs` | `SILLAGE1` + `1u32` LE + `postcard((corpus, index, bm25))`. Writes go to `<file>.tmp` and are then renamed into place. `load` checks the magic and version with clear errors. |

## Tests (`tests/`, none use `sillage-parse`)

- `common/mod.rs`: a small tree builder that computes labels with `make_label`, assigns keyword slot 0 and 1-based non-keyword slots, and runs `validate()`. It also holds a hand-built Fig. 4 / Listing 4 tree.
- `paper_features.rs`: exact Table 2 rows for `view_30` and `0_20`; the first and last leaves; the member context (`x.foo()`) and its fallback; alpha-renaming invariance; `featurize` counts equal to `feature_debug`.
- `index_oracle.rs`: a proptest and a seeded ChaCha8 run (2000 methods) against a brute-force `BTreeSet` intersection oracle. Also covers set semantics, edge cases, duplicate-ID merging, and an Aroma self-query ranking first.
- `bm25_store.rs`: ranking, sub-word matching, body-only indexing, empty and zero-n cases, tie-breaking, the idf and length-norm direction, a save/load roundtrip with `tempfile`, and bad magic, version, truncated and missing files.
- Unit tests in `hash.rs`, `bm25.rs` and `corpus.rs`.

## Paper notes

- Table 2 prints the third parent of `0_20` as `#;#;#;`. Fig. 4 and Def. 4 applied to `init ; cond ; update` give `#;#;#`, so the test asserts `#;#;#`.
- How the `Member` context is displayed in `feature_debug` is my choice: the bare token text, e.g. `(foo, (1, #=#))`.
