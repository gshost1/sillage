# sillage — Architecture & Design Specification

*Sillage* is the trail of scent a perfume leaves behind. This project is a clean-room Rust remake of **Aroma: Code Recommendation via Structural Code Search** (Sifei Luan, Di Yang, Celeste Barnaby, Koushik Sen, Satish Chandra — OOPSLA 2019, [arXiv:1812.01158](https://arxiv.org/abs/1812.01158)).

## Stack

- **Language:** Rust (single Cargo workspace)
- **Parsing:** `tree-sitter` with statically linked grammars for Java, Python, JavaScript, TypeScript/TSX, Go, and Rust
- **Parallelism:** `rayon`
- **Feature Hashing:** `xxhash-rust` (`xxh3`) for stable 64-bit feature IDs on disk
- **Hot-loop Maps:** `rustc-hash`
- **Persistence:** `serde` + `postcard`
- **CLI:** `clap` (derive)
- **Diff Highlighting:** `similar`
- **Corpus Walking:** `ignore`
- **Sampling / Bootstrap:** `rand` + `rand_chacha`
- **Tests:** `proptest`, `insta`

## Crate Dependency Graph

```text
sillage-core
   ▲
sillage-parse
   ▲
sillage-index
   ▲
sillage-engine
   ▲          ▲
sillage-render ◄── sillage-bench
      ▲          ▲
      sillage-cli
```

Every crate depends on `sillage-core`. `sillage-bench` depends on `sillage-render`
(the quality bench compares *rendered* recommendation lines with the held-out
half). `sillage-engine` only touches `sillage-parse` and `sillage-index` inside
`pipeline.rs`.

## Paper Clarifications & Decisions (implemented in `sillage-core::params::Params`)

1. **`s(τ)` denominator:** The paper writes `s(τ) = csq(τ) / |F(N2(i1))|`. Because `cs ≤ |F(N2(i1))|` and `l(τ) = cs/csq > 1.5`, `csq / |F(N2(i1))| < 1/1.5 ≈ 0.67`, making `s(τ) > 0.9` impossible. The paper's prose says the intersection of pruned snippets should be "very similar to the first *pruned* snippet", so the default denominator is `|F(Prune(F(q), N2(i1)))|` (`SDenom::PrunedFirst`), with `SDenom::PaperLiteral` available as a toggle.
2. **Intersect operators:** Pair uses multiset sum (`⊎`); 3+ uses multiset union/max (`∪`), matching Section 3.3.3, with a toggle (`IntersectOp::AsPaper` / `AlwaysSum` / `AlwaysUnion`).
3. **Cluster growth:** `∀j > i_k` ranges over all `j` in `i_k + 1 ..= |N2|`, not just valid ones (the max `l` is taken before the validity check); stop when `C_ℓ = C_{ℓ+1}`. Which maximisers extend a tuple is Decision 13: by default only the earliest (`ClusterTies::First`); the paper's every-tied-`i` rule is `ClusterTies::All`.
4. **Duplicate clusters:** Jaccard similarity of tuple index sets compared against all earlier tuples in the sorted list (`Dedup::AllEarlier`, literal reading), with `Dedup::KeptOnly` toggle.
5. **Phase I vs Phases II–III:** Phase I uses set semantics (`S(F)`); Phases II and III use multiset semantics (`F`).
6. **`τ1` threshold:** Applied to `SimScore(F(q), F(Prune(F(q), m))) / |F(q)|` (default `0.65`).
7. **Features inside vs after `Prune`:** Inside `Prune`, `F(n)` is each leaf's feature bag in the original `m2`. Whenever the paper writes `F(mp)` (`csq`, `Intersect`), the pruned tree is built and re-featurized.
8. **Free identifiers (revised in Stage 2):** Undeclared identifiers in value position count as locals (`#VAR`), matching Fig. 4 (`view`), in query snippets **and** in corpus methods. Callees, members, types, labels, capitalised names and builtins do not. (The original asymmetric rule made every field read `#VAR` in a query but stay a global in its method; fixing it raised contiguous Recall@1 by ~6.5 points.)
9. **Recall@1 hit (§5 "100% similar"):** §5: "Aroma should rank the original method body as number 1 in the reranked search result list, or the original method body should be 100% similar to the first code snippet in the ranked results." **§5 R@1** counts a hit when (a) the original is ranked first; or (b) the rank-1 method has exactly the original's feature multiset `F(orig)` (an undeduplicated copy, e.g. a renamed method), for every system; or (c) for Sillage, the original ties rank 1 at the full query similarity `|F(q)|`, so both contain the whole query and the query cannot separate them. BM25 and feature search have no score comparable to `|F(q)|`, so (c) does not apply to them. Partial ties (`sim < |F(q)|`) earn nothing. **Strict R@1** (original ranked first; Sillage orders equal-similarity candidates by Decision 24, the baselines by `MethodId`) is reported next to it for every system and is the like-for-like comparison. Before Stage 5, any equal-similarity tie counted, for Sillage only.
10. **Corpus trees are method bodies:** a method is parsed whole (so parameters scope as locals) but its tree is the body subtree (`parse_method(lang, text, body)`); signatures never reach features or recommendations.
11. **C-style `for` headers** are grouped into one `#;#;#` node (`for(#)#`, Fig. 4) and a declaration initialiser's own `;` is hoisted into it (`int#`, Table 2).
12. **Size-1 clusters** recommend the full method body (`Singleton::FullBody`, §4.3.5) rather than §3.3.3's `Prune(F(q), N2(i1))` (`Singleton::PrunedToQuery`).
13. **Cluster growth ties:** only the earliest maximiser extends a tuple (`ClusterTies::First`, default); the literal all-maximisers rule (`ClusterTies::All`) is exponential on repetitive code. Growth runs in chunks of the frontier and stops at exactly `MAX_TUPLES` (5,000) tuples, even inside one level. Jaccard dedup first applies a cheap necessary condition (`min(|a|,|b|)/max(|a|,|b|) > threshold` and overlapping index ranges).
14. **Statement-sequence normalisation:** a parent / usage context whose label is a statement sequence (`{###}` or brace-less `###`) is featurized as `(n, 0, {#})` (`AromaFeaturizer`); `PaperFeaturizer` keeps literal labels.
15. **Partial snippets:** missing closers (`)`, `]`, `}`) are appended and kept as zero-width keywords, as are tree-sitter `MISSING` tokens; interpolated strings (JS templates, f-strings) are lowered structurally.
16. **Rendering:** a recommendation's outer body braces are not shown; token-less subtrees (`{}`, `[]`) survive pruning; a dropped callee / member name of a kept call is restored (`Params::complete_calls`).
17. **§5 sampling** counts code lines only (lines touched by a terminal of the parsed body), so comment lines never form queries. Interior lines of a multi-line string (docstrings, Java text blocks, raw strings, multi-line template literals) are skipped too, uniformly for every language.
18. **Query-mode features** (`Featurizer::featurize_query`, `AromaFeaturizer`): a partial snippet emits sibling and variable-usage features only between leaves on the same line, and no parent feature whose ancestor is a statement-sequence root. Cross-line pairs from a non-contiguous sample are evidence for a method where those lines *are* adjacent. Corpus methods use the full featurization; `PaperFeaturizer` keeps the literal rule for both.
19. **Snippet holes:** a featureless `#` node (an internal node labelled `""` with one zero-width keyword) is inserted after a trailing dangling operator (`x =`, `a &&`, `obj.`) and before a leading-dot chain line whose receiver was not sampled (`.build();`), so the parent label reads like complete code. A trailing `,` gets no hole. The bracket scanner that finds them skips comments, strings (with `\\` escapes in single- and double-quoted strings), char literals and Python / Java `"""` blocks.
20. **Bounded Phase II:** `rerank_top` loads candidates in `(bound DESC, overlap DESC, stored |F(m)| ASC, id ASC)` order, where bound is the Phase I multiset bound `Σ_f min(q_f, m_f)` (posting counts saturate at 255). It skips a candidate whose optimistic Decision 24 key cannot beat the η2-th key, and stops at the first bound below the η2-th similarity (or not above `τ1`). A candidate whose `(bound, overlap)` *equals* the η2-th `(sim, overlap)` can still win on the tie-break fields. For it the engine computes `Σ_f min(F_full(q)_f, m_f)` from the postings (an upper bound on `full_sim`, capped at `|F_full(q)|`) and uses the stored `|F(m)|` (a lower bound on `size`). It loads the candidate only if that optimistic key still wins. The output equals the exhaustive rerank truncated to η2 (proptest `rerank_top_matches_exhaustive`, 1,024 cases with heavy ties and none / exact / loose tie bounds; e2e `single_pass_index_and_bounded_rerank_are_exact`).
21. **Language families:** Phase I keeps only methods whose language is in the query's family (`lang_compatible`: same language, except TypeScript and TSX share one) *before* the η1 cut, so in a mixed-language index other languages never take candidate slots and clusters never mix languages. BM25 and feature search apply the same filter.
22. **Leave-one-out quality:** `Engine::recommend_excluding` removes methods before the η1 cut. The §4.2 quality bench excludes the query's own method this way, so it never occupies an N2, cluster, dedup or top-k slot. Containment (`≥ τ1`) uses the query-mode `F(q)`, like the engine; added-feature precision / recall use the full featurization on both sides.
23. **BM25 tokens** are case-sensitive whole identifiers (`getChildCount`, `HTTPServer_v2`); the Stage 3 ablation found sub-word splitting dilutes IDF.
24. **Phase II tie-break (Stage 6):** candidates are ranked by `TieKey = (sim DESC, overlap DESC, full_sim DESC, size ASC, id ASC)`. `sim` is the pruned similarity, Aroma's ranking, which is unchanged; the other keys only order equal-`sim` candidates. `overlap` is the Phase I set overlap, the Stage 5 secondary key. `full_sim = |F_full(q) ∩ F(m)|` uses the query's full featurization (`Featurizer::featurize`), whose cross-line sibling and variable-usage features query mode drops (Decision 18): among methods that contain the same query-mode features, it prefers the one whose lines are laid out as in the snippet. `size = |F(m)|` prefers the tighter method, with fewer features outside the query. Before Stage 6, ties on `(sim, overlap)` (every full-query tie) fell through to `MethodId`. All keys are computed from alpha-renamed features, so the ranking is invariant under renaming locals (e2e `sim_ties_break_structurally_and_alpha_invariantly`); raw identifier names are never used. Of the orders measured on the development seed 7 (`full_sim` alone, `size` alone, both, and with or without `overlap` first), `full_sim` then `size` was best. `overlap`-first equals `full_sim`-first on non-contiguous queries and is 0.1 points behind on contiguous ones. It was kept because it only refines the Stage 5 order and prunes more. Seed 2027 was not used for tuning.

## Index format (v5)

`b"SILLAGE1"` magic, `u32` LE format version (5), `u64` LE `xxh3_64` of the payload, then `postcard((Corpus, InvertedIndex, Bm25))`. `load` checks the magic, rejects older versions with a rebuild hint, verifies the checksum before decoding, and then validates:

- `Corpus`: `methods[i].id == MethodId(i)`; every body span satisfies `start <= end <= text.len()` on UTF-8 character boundaries;
- `InvertedIndex`: `offsets.len() == feature_ids.len() + 1`, offsets start at 0 and are non-decreasing, the last offset equals `postings.len()`, `counts.len() == postings.len()`, feature ids strictly increasing, every posting `< num_methods`, `num_methods == |corpus|`, `sizes.len() == num_methods` (v5: per-method `|F(m)|`, 0 for an id built from several bags, so it stays a lower bound for Decision 20);
- `Bm25`: one document (and length) per corpus method, offsets consistent with terms and postings, postings and document ids in range, finite `avgdl`.

A corrupted, truncated or hand-edited file is an error, never a panic (`bm25_store::load_rejects_flipped_bytes_without_panicking`, `corrupted_payload_with_valid_checksum_never_panics`). CSR construction uses checked `u32` conversions, and Phase I overlap accumulators are `u32`.

## Work Slices

- **Step 0 (Lead):** Workspace scaffold, frozen `sillage-core`, compiling public API stubs across all crates.
- **Slice A (`crates/sillage-parse`):** Tree-sitter parsing, simplified parse tree lowering, lexical scoping, method extraction, error-tolerant snippet parsing.
- **Slice B (`crates/sillage-index`):** 4 Aroma structural feature families, CSR inverted index (`D · v_q`), BM25 keyword baseline, corpus walker with project/file/method hash dedup, postcard persistence.
- **Slice C (`crates/sillage-engine`):** Lazy-greedy Algorithm 1 `prune`, parallel `rerank`, constrained `cluster`, recursive `intersect`, end-to-end `Engine` pipeline.
- **Slice D (`crates/sillage-render`, `crates/sillage-bench`, `crates/sillage-cli`, `corpus.toml`, `bench/queries`):** Source-preserving snippet rendering (Plain/ANSI/HTML/JSON), §5 Recall@1/Recall@100 bootstrap benchmark, §4.2 Quality benchmark, CLI binary, corpus manifest.
