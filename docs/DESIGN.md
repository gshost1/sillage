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
sillage-render  sillage-bench
      ▲          ▲
      sillage-cli
```

(`sillage-engine` only touches `sillage-parse` and `sillage-index` inside `pipeline.rs`.)

## Paper Clarifications & Decisions (implemented in `sillage-core::params::Params`)

1. **`s(τ)` denominator:** The paper writes `s(τ) = csq(τ) / |F(N2(i1))|`. Because `cs ≤ |F(N2(i1))|` and `l(τ) = cs/csq > 1.5`, `csq / |F(N2(i1))| < 1/1.5 ≈ 0.67`, making `s(τ) > 0.9` impossible. The paper's prose says the intersection of pruned snippets should be "very similar to the first *pruned* snippet", so the default denominator is `|F(Prune(F(q), N2(i1)))|` (`SDenom::PrunedFirst`), with `SDenom::PaperLiteral` available as a toggle.
2. **Intersect operators:** Pair uses multiset sum (`⊎`); 3+ uses multiset union/max (`∪`), matching Section 3.3.3, with a toggle (`IntersectOp::AsPaper` / `AlwaysSum` / `AlwaysUnion`).
3. **Cluster growth:** `∀j > i_k` ranges over all `j` in `i_k + 1 ..= |N2|`, not just valid ones; ties add every tied `i`; stop when `C_ℓ = C_{ℓ+1}`.
4. **Duplicate clusters:** Jaccard similarity of tuple index sets compared against all earlier tuples in the sorted list (`Dedup::AllEarlier`, literal reading), with `Dedup::KeptOnly` toggle.
5. **Phase I vs Phases II–III:** Phase I uses set semantics (`S(F)`); Phases II and III use multiset semantics (`F`).
6. **`τ1` threshold:** Applied to `SimScore(F(q), F(Prune(F(q), m))) / |F(q)|` (default `0.65`).
7. **Features inside vs after `Prune`:** Inside `Prune`, `F(n)` is each leaf's feature bag in the original `m2`. Whenever the paper writes `F(mp)` (`csq`, `Intersect`), the pruned tree is built and re-featurized.
8. **Free identifiers (revised in Stage 2):** Undeclared identifiers in value position count as locals (`#VAR`), matching Fig. 4 (`view`), in query snippets **and** in corpus methods. Callees, members, types, labels, capitalised names and builtins do not. (The original asymmetric rule made every field read `#VAR` in a query but stay a global in its method; fixing it raised contiguous Recall@1 by ~6.5 points.)
9. **Recall@1 hit:** Counts as a hit when the original method is ranked first, OR when the top result's pruned similarity equals the original's (matching Section 5's "100% similar" clause).
10. **Corpus trees are method bodies:** a method is parsed whole (so parameters scope as locals) but its tree is the body subtree (`parse_method(lang, text, body)`); signatures never reach features or recommendations.
11. **C-style `for` headers** are grouped into one `#;#;#` node (`for(#)#`, Fig. 4) and a declaration initialiser's own `;` is hoisted into it (`int#`, Table 2).
12. **Size-1 clusters** recommend the full method body (`Singleton::FullBody`, §4.3.5) rather than §3.3.3's `Prune(F(q), N2(i1))` (`Singleton::PrunedToQuery`).
13. **Cluster growth ties:** only the earliest maximiser extends a tuple (`ClusterTies::First`); the literal all-maximisers rule (`ClusterTies::All`) is exponential on repetitive code and is capped at `MAX_TUPLES`.
14. **Statement-sequence normalisation:** a parent / usage context whose label is a statement sequence (`{###}` or brace-less `###`) is featurized as `(n, 0, {#})` (`AromaFeaturizer`); `PaperFeaturizer` keeps literal labels.
15. **Partial snippets:** missing closers (`)`, `]`, `}`) are appended and kept as zero-width keywords, as are tree-sitter `MISSING` tokens; interpolated strings (JS templates, f-strings) are lowered structurally.
16. **Rendering:** a recommendation's outer body braces are not shown; token-less subtrees (`{}`, `[]`) survive pruning; a dropped callee / member name of a kept call is restored (`Params::complete_calls`).
17. **§5 sampling** counts code lines only (lines touched by a terminal of the parsed body), so comment lines never form queries.

## Work Slices

- **Step 0 (Lead):** Workspace scaffold, frozen `sillage-core`, compiling public API stubs across all crates.
- **Slice A (`crates/sillage-parse`):** Tree-sitter parsing, simplified parse tree lowering, lexical scoping, method extraction, error-tolerant snippet parsing.
- **Slice B (`crates/sillage-index`):** 4 Aroma structural feature families, CSR inverted index (`D · v_q`), BM25 keyword baseline, corpus walker with project/file/method hash dedup, postcard persistence.
- **Slice C (`crates/sillage-engine`):** Lazy-greedy Algorithm 1 `prune`, parallel `rerank`, constrained `cluster`, recursive `intersect`, end-to-end `Engine` pipeline.
- **Slice D (`crates/sillage-render`, `crates/sillage-bench`, `crates/sillage-cli`, `corpus.toml`, `bench/queries`):** Source-preserving snippet rendering (Plain/ANSI/HTML/JSON), §5 Recall@1/Recall@100 bootstrap benchmark, §4.2 Quality benchmark, CLI binary, corpus manifest.
