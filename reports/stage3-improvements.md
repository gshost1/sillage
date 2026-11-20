# Stage 3: accuracy, multi-language Table 5, quality and CLI polish

Date: 2026-11-20. Machine: laptop (8 cores, 14 GB RAM). Branch `main`.

- **Before** is Stage 2 (`8f8afc3`, `reports/integration-bench-v1.md`).
- **After** is Stage 3 (this report's commits, on top of the perf commit `096125a`).

All numbers come from the release binary with seed 7, n = 1000 recall queries, bootstrap 1000 and n = 200 quality queries, unless noted.

## Summary

- **Java non-contiguous Sillage R@1: 91.5% → 95.2%.** Contiguous: 98.9% → **99.1%** (paper: 98.3% / 99.1%). The Java query sets are **byte-identical** to Stage 2: the fingerprints match, the physical-line sampler is unchanged, and the Java corpus is the same 80,192 methods.
- **Root cause of the misses.** On every Java miss (n = 1000, both modes), the original's pruned similarity equals the ideal `|F(q) ∩ F(orig)|`, so greedy prune never loses features. The misses happen because the query has features the original lacks and a near-duplicate has. Most of these are **sibling and variable-usage pairs that span two sampled lines**. Rerank tie-breaking cannot change R@1: Decision 9 already counts equal-similarity ties as hits, and every miss is a strict loss.
- **Table 5 is new.** Each non-Java language now has 3.7k–9.3k methods (9 pinned repos added). Table 5 found real bugs. **Python was broken: R@1 56.3% (Stage 2) → 92.8%.** Queries were sampled from inside docstrings and triple-quoted fixtures. Rust contiguous: 95.4% → 99.0%.
- **BM25:** case-sensitive whole identifiers. Java R@1 73.2% → 74.0% contiguous, 81.0% → 81.9% non-contiguous (paper: 78.3% / 93.0%). Index format v3.
- **Quality:** new metrics for cluster sizes and line-level exact match. Most recommendations are still full-body singletons (427 of 458 on Java). The distinct-feature cluster guard was tested and **not adopted**: it made no measurable difference.
- **CLI:**
  - `query --format json` has file line numbers and cluster-member metadata;
  - `query --max-lines N` truncates long recommendations;
  - no more dangling `view.` receivers.
- **Tests:** `cargo test --workspace` gives **140 passed, 0 failed, 1 ignored** (the ignored test is the on-corpus check, run by hand). `cargo build --release --bins --examples` gives **0 warnings**.

## Java, paper Table 4 (80,192 methods, n = 1000, seed 7)

`sillage bench recall --index data/java.sil --n 1000 --mode {contiguous,noncontiguous} --bootstrap 1000 --seed 7`

Query-set fingerprints (`examples/misses.rs`) are `cdb10bf0fdb864a0` (contiguous) and `b1aabba2562f9058` (non-contiguous), identical before and after.

| System | Mode | Stage 2 R@1 | **Stage 3 R@1** [95% CI] | Stage 3 R@100 | Paper R@1 / R@100 |
|---|---|---|---|---|---|
| BM25 | contig | 73.2% | **74.0%** [71.2, 76.8] | 99.5% | 78.3% / 96.9% |
| Features (Phase I) | contig | 83.4% | 81.1% [78.5, 83.7] | 99.9% | 78.3% / 96.8% |
| **Sillage** | contig | 98.9% | **99.1%** [98.5, 99.6] | **100.0%** | 99.1% / 100% |
| BM25 | non-contig | 81.0% | **81.9%** [79.5, 84.3] | 100.0% | 93.0% / 99.9% |
| Features (Phase I) | non-contig | 80.2% | 79.5% [77.1, 82.0] | 99.1% | 88.1% / 98.6% |
| **Sillage** | non-contig | 91.5% | **95.2%** [93.8, 96.5] | 99.1% | 98.3% / 100% |

Features Search R@1 drops by 2.3 / 0.7 points. It ranks by Phase I set overlap on the new query-mode F(q), which has fewer cross-line relations. Sillage itself, which reranks on that same F(q), gains.

### What moved Java recall (Sillage R@1, checked on seeds 7 and 11)

| Step | contig s7 | non-contig s7 | contig s11 | non-contig s11 |
|---|---|---|---|---|
| Stage 2 | 98.9 | 91.5 | 98.1 | 92.9 |
| Query siblings not crossing top-level statements | 98.9 | 92.4 | | |
| + var-usage not crossing top-level statements | 98.9 | 93.0 | | |
| + no parent feature at the query's root | 98.9 | 93.3 | | |
| Siblings / var-usage only within a **line**, root dropped always | 99.0 | 95.0 | 98.0 | 96.0 |
| **Query mode as committed** (root dropped only when it is a statement sequence) | 98.9 | 95.0 | 98.0 | 95.5 |
| + Java constructor wrapper (`this(…)` / `super(…)`) | 99.1 | 95.0 | 98.3 | 95.6 |
| + leading-dot holes only (`.build();`) | 98.9 | 95.2 | 98.0 | 95.7 |
| + trailing-operator holes, **including** `,` | 98.7 | 94.9 | 97.6 | 95.5 |
| **Final:** ctor + leading-dot + trailing-operator holes **excluding** `,` | **99.1** | **95.2** | **98.3** | **95.8** |

Other findings from the same runs:

- Dropping sibling and usage features across *all* nested statement boundaries (`sibdeep`) gave 98.8 / 93.9 on seed 7. That is worse than the per-line rule.
- Statement-sampled queries (the optional secondary mode) were **not** implemented. All numbers above use the paper-style physical-line sampler.

### Fixes (all have regression tests)

1. **Query-mode featurization.** `Featurizer::featurize_query` defaults to `featurize`, so `PaperFeaturizer` stays literal. `AromaFeaturizer`'s query mode:
   - emits sibling and variable-usage features only between leaves **on the same line**;
   - skips parent features whose ancestor is a statement-sequence root.

   Corpus features are unchanged, so no reindex was needed for this. The cross-line pair `(4, verify)`, from the end of one sampled line and the start of the next, is evidence for the near-duplicate where those lines *are* adjacent. Diagnosed with `examples/misses.rs` (`SILLAGE_DIFF=1`). Tests: `paper_features::query_mode_drops_cross_line_relations`.
2. **Snippet holes.** A featureless `#` node: an internal node labelled `""` with one empty zero-width keyword. Its parent's label reads like the complete code, and it adds no features. Holes are inserted:
   - after a trailing dangling operator (`x =` → `#=#`, `a &&` → `#&&#`, `obj.` → `#.#`);
   - before a leading-dot chain line whose receiver was not sampled (`.build();` → `#.#` / `#()`).

   A **trailing `,` gets no hole.** Measured, it cost contiguous recall: `foo(a, b,` would match *other* methods' 3-argument calls exactly, while the cut call's arity is unknown. Left as is, `#(#,#,)` matches nothing. Tests: `snippets::{dangling_operators_get_holes, trailing_comma_gets_no_hole, leading_dot_lines_get_hole_receivers}`.
3. **Java constructor wrapper** (`class __S { __S() { … } }`), so `this(…)` and `super(…)` parse error-free with the corpus constructor's shape. Test: `explicit_constructor_calls_parse_in_constructor_wrapper`.
4. **Prune and rerank:** no change. Greedy prune was optimal on every miss, and tie-breaking is irrelevant under Decision 9 (see the summary). Prune keeps its proptest equivalence with the naive version.

## Multi-language Table 5 (n = 1000, seed 7)

Corpora: `corpus.toml`, two repos per language in Stage 2 plus two more added in Stage 3:

| Language | Repos | Methods |
|---|---|---|
| Python | requests, click, **pytest 8.3.3, rich 13.9.4** | 8,554 |
| JavaScript | express, **eslint 9.12.0, prettier 3.3.3** | 9,347 |
| TypeScript | zod, **typeorm 0.3.20, nest 10.4.4** | 5,592 |
| Go | cobra, mux, **gin 1.10.0, caddy 2.8.4** | 3,729 |
| Rust | ripgrep, serde_json, **tokio 1.40.0** | 8,677 |

The "Stage 2" columns run the Stage 2 binary (`8f8afc3`, its own sampler and format v1) on the same Stage 3 corpora. For non-Java languages the Stage 3 sampler skips string interiors (fix 1 below), so the two query sets differ. They are end-to-end before/after numbers, not the same queries.

| Language | Mode | Stage 2 Sillage R@1 / R@100 | **Stage 3 Sillage R@1 / R@100** | Stage 3 BM25 R@1 | Stage 3 Features R@1 | Stage 2 wall | **Stage 3 wall / RSS** |
|---|---|---|---|---|---|---|---|
| Python | contig | 56.3% / 63.4% | **92.8% / 96.7%** | 79.1% | 76.9% | 82 s | 9.0 s / 94 MB |
| Python | non-contig | 66.1% / 76.2% | **92.1% / 95.5%** | 88.0% | 83.5% | 103 s | 9.5 s / 90 MB |
| JavaScript | contig | 96.2% / 99.5% | **96.3% / 99.5%** | 81.8% | 67.6% | 378 s | 80 s / 337 MB |
| JavaScript | non-contig | 87.2% / 97.4% | **89.4% / 96.6%** | 80.6% | 62.7% | 364 s | 61 s / 300 MB |
| TypeScript | contig | 98.3% / 99.9% | **98.4% / 100%** | 69.6% | 73.3% | 222 s | 43 s / 175 MB |
| TypeScript | non-contig | 88.3% / 98.1% | **91.5% / 98.7%** | 78.4% | 70.3% | 213 s | 54 s / 196 MB |
| Go | contig | 99.3% / 99.7% | **99.6% / 100%** | 81.0% | 86.1% | 223 s | 30 s / 134 MB |
| Go | non-contig | 93.5% / 97.1% | **96.3% / 98.4%** | 88.7% | 84.0% | 205 s | 29 s / 132 MB |
| Rust | contig | 95.4% / 95.9% | **99.0% / 99.4%** | 77.1% | 84.3% | 145 s | 18 s / 90 MB |
| Rust | non-contig | 86.3% / 93.7% | **92.9% / 97.2%** | 80.4% | 79.3% | 143 s | 22 s / 95 MB |
| *Java (Table 4)* | contig / non-contig | 98.9% / 91.5% | **99.1% / 95.2%** | 74.0% / 81.9% | 81.1% / 79.5% | 154 / 167 s | 16.3 / 15.9 s, 237 / 250 MB |

I did not re-check paper Table 5's per-language values for this report, so no paper column is shown.

### Language bugs Table 5 found (fixed, with tests)

1. **Python R@100 was 63%: the sampler read docstrings as code.** `code_lines` counted every line a multi-line string touches, so queries started inside docstrings and `pytester.makepyfile("""…""")` fixtures, and the parser read prose as code. Interior lines of a multi-line string are now skipped like comments. This covers both atomic strings and structural f-strings / template literals (first child is the opening quote).
   - It applies to all languages **except Java**, to keep Java's seeded query sets byte-identical to Stage 2.
   - In Java the rule would touch exactly one of 80,192 methods (a JDK 21 text-block test, 12 → 7 code lines). That single eligibility change reshuffles 373 of the 1000 seeded picks.
   - Effect: Python 56.2% → 93.2% contiguous; Rust 95.5% → 99.0% (ripgrep's raw-string help texts).
   - Test: `sample::code_lines_skip_multiline_string_interiors`.
2. **A lone triple-quote line swallowed the rest of the query.** In non-contiguous queries, a sampled `"""` line (one end of a multi-line string) opened a string that ran to the end of the snippet. Python snippets with an **odd** total of triple quotes first try a variant with the odd lines blanked (same length, so spans are unchanged). Blanking whenever any odd line exists cost 5 points of contiguous recall, because pairs `x = """` … `"""` were broken. Python non-contiguous: 84.1% → 92.1% (with f-strings). Test: `snippets::python_open_triple_quote_lines_are_blanked`.

### Known limits (not fixed)

- **Small-corpus search latency.** On 4–9k-method corpora the 100th-best similarity is low, so the bounded Phase II cannot prune: about 1000 candidates are re-parsed per query (`examples/profile.rs`). That gives 30–80 s per 1000 queries, against 16 s on the 80k Java corpus. A parsed-tree cache, which is cheap at this size, would fix it.
- **Generated JS fixtures.** eslint's `tests/fixtures/parsers/*.js` are AST dumps; the largest "method" is 331 KB / 8,902 lines. They inflate JS "extra lines per recommendation" (199.7) and Phase II time. Excluding them would need a corpus filter, which I did not add so as not to curate the benchmark.
- JS and TS non-contiguous misses are mostly near-duplicates at rank 2 (32 of 106 JS misses).

## BM25 tokenizer (Java, n = 1000, seed 7; `examples/bm25.rs`)

| Tokenizer | contig R@1 | non-contig R@1 | Terms |
|---|---|---|---|
| Lower-cased whole + camelCase/snake sub-words (Stage 2) | 73.2% | 81.0% | 101,095 |
| Lower-cased whole only | 73.3% | 81.0% | 98,634 |
| Sub-words only | 68.9% | 79.5% | 35,878 |
| **Case-sensitive whole (Stage 3)** | **74.0%** | **81.9%** | 105,726 |

Sub-words dilute IDF with common fragments (`get`, `set`, `test`). The gap to the paper (78.3% / 93.0%) is most likely the corpus: our ExoPlayer and guava test suites are full of near-duplicate methods.

## Recommendation quality (Java, half-snippet queries, n = 200, seed 7)

`sillage bench quality --index data/java.sil --n 200 --seed 7`

| Metric | Stage 2 | **Stage 3** | Stage 3 without query mode |
|---|---|---|---|
| Containment rate (≥ τ1) | 70.5% | 70.0% | 71.0% |
| Avg precision of added features | 49.5% | 48.2% | 49.5% |
| Avg recall of held-out features | 67.2% | 67.2% | 67.2% |
| Avg recommendations / query | 2.10 | 2.29 | 2.12 |
| Avg extra lines / recommendation | 25.8 | 24.9 | 25.7 |
| **Exact-match rate** (added lines ⊆ held-out) | n/a | 3.5% | 3.5% |
| **Line precision / recall** (best rec) | n/a | 40.9% / 51.7% | 41.3% / 51.9% |
| **Queries with a multi-method cluster** | n/a | 13.5% | 12.0% |
| **Cluster sizes** (1 / 2 / 3–5 / 6+) | n/a | 427 / 23 / 7 / 1 | 394 / 21 / 6 / 2 |

- Query mode returns more recommendations and more multi-method clusters, at −1.3 points of feature precision. That is within the n = 200 noise (about ±2 points). I kept one F(q) for search and recommendation.
- **Most recommendations are full-body singletons (93%).** That is why the exact-match rate is low: leave-one-out removes the original, and a singleton is another method's whole body.

Other languages (Stage 3, n = 200):

| Language | Containment | Feature P / R | Exact match | Line P / R | Multi-cluster queries | Sizes (1 / 2 / 3–5 / 6+) | p50 / p95 / max |
|---|---|---|---|---|---|---|---|
| Python | 52.0% | 33.2% / 48.1% | 1.0% | 17.9% / 32.6% | 14.0% | 359 / 21 / 4 / 11 | 1.3 / 9.7 / 24.3 ms |
| JavaScript | 71.5% | 25.4% / 65.5% | 2.5% | 23.9% / 55.3% | 7.5% | 500 / 16 / 1 / 0 | 12.7 / 51.5 / 79.4 ms |
| TypeScript | 58.0% | 40.6% / 64.2% | 2.5% | 43.6% / 56.5% | 22.0% | 328 / 41 / 8 / 2 | 1.9 / 36.2 / 53.0 ms |
| Go | 49.5% | 34.5% / 59.7% | 0.0% | 30.1% / 46.0% | 10.0% | 278 / 15 / 8 / 0 | 1.9 / 18.0 / 50.2 ms |
| Rust | 58.0% | 50.6% / 67.6% | 1.5% | 41.7% / 52.1% | 17.0% | 336 / 30 / 7 / 1 | 2.5 / 11.0 / 26.8 ms |

### Heterogeneous-cluster guard: tested, not adopted

The guard additionally required each cluster extension's ratio over **distinct** features, `|set(cs)| / |set(csq)|`, to exceed τ2. That stops a cluster growing on repeated generic features such as `checkNotNull(#VAR)` multiplied many times.

- Java quality: identical containment, precision, recall and exact match.
- Effect: 4 fewer multi-method clusters (multi-cluster queries 13.5% → 12.0%).
- `checknotnull.java` still got 2-member clusters, and `equals.java` was unchanged.

It did not measurably improve quality, so it was reverted. Listing 4 → 7 and `equals` clustering are unaffected either way.

## Latency, indexing and memory (Java, 80k methods)

| | Stage 2 | **Stage 3** |
|---|---|---|
| `sillage index --lang java` | 10.6 s, 572 MB | **7.1 s, 605 MB** (format v3; same 80,192 methods, 883,117 features, 5,288,669 postings) |
| Recall bench contig / non-contig (wall) | 154 s / 167 s | **16.3 s / 15.9 s** |
| Recall bench peak RSS | 1.2 GB | **237 MB / 250 MB** |
| Quality bench (200 queries, wall) | 37 s | **3.2 s** |
| `recommend` p50 / p95 / max | 147.2 / 210.2 / 237.5 ms | **2.7 / 17.6 / 43.2 ms** |
| Phase I p50 / p95 / max | 132.1 / 187.1 / 211.3 ms | 0.8 / 1.2 / 1.8 ms |
| Phase II p50 / p95 / max | 14.1 / 23.6 / 28.0 ms | 1.1 / 12.7 / 39.1 ms |
| Phase III p50 / p95 / max | 0.5 / 3.7 / 9.0 ms | 0.4 / 3.8 / 6.5 ms |

Most of the latency gain comes from the perf commit `096125a` (bounded Phase II, postings with counts). Stage 3's changes add no measurable latency.

Indexing the other languages takes 0.7–2.0 s each, at 79–213 MB.

The Stage 2 `examples/misses.rs` held 1000 × 100 full `Ranked` values, with two trees each. It used **6 GB RSS** and caused the earlier OOM. It now keeps `(id, sim, overlap)` and runs in 16 s at 258 MB.

## Rendering and CLI

- `sillage query --format json`:
  - every rendered line has `file_line` (file line number), computed from the first cluster member's `MethodRecord.line_start`. The recommendation tree is derived from that member's text. Verified: jsoup `Range.equals` line 100 maps to `if (this == o) return true;`;
  - `cluster` is a list of `{id, repo, path, name, line_start}` objects.
- `sillage query --max-lines N` truncates each recommendation in every format. The last line is `// ... (K more lines)` (`... (K more lines)` for Python), indented like the last kept line. API: `Layout::{locate, truncate}`, `render_layout`.
- **Dangling receivers:** a dropped member after `.` / `?.` / `::` that is too large to restore (a whole regrouped call) gets its head token back. `view.` now renders as `view.setOnTouchListener(` followed by a placeholder. Test: `toggles::complete_calls_restores_member_head_after_dot`. Listing 4 → 7 is still exact (`e2e`).
- `sillage explain` shows query-mode features (`query_feature_debug`).

## Tools added

- `examples/misses.rs`. Usage: `misses <index> <n> <mode> <seed> [limit]`.
  - Prints the R@1-miss histogram and a query-set fingerprint.
  - `SILLAGE_DIFF=1` shows the query features each miss's original lacks, with the ideal similarity.
  - `SILLAGE_DUMP=<path>` writes the query set.
- `examples/bm25.rs`. Gives BM25 R@1/R@100 with the BM25 index rebuilt by the current tokenizer.

## Reproduce

```sh
cd /home/george/Projects/sillage
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR=$PWD/target TMPDIR=$PWD/target/tmp CARGO_BUILD_JOBS=4; mkdir -p $TMPDIR
cargo build --release --bins --examples
target/release/sillage fetch --manifest corpus.toml --out data/corpus
for l in java python javascript typescript go rust; do target/release/sillage index --corpus data/corpus --out data/$l.sil --lang $l; done
for l in java python javascript typescript go rust; do for m in contiguous noncontiguous; do
  target/release/sillage bench recall --index data/$l.sil --n 1000 --mode $m --bootstrap 1000 --seed 7; done; done
target/release/sillage bench quality --index data/java.sil --n 200 --seed 7
SILLAGE_DIFF=1 target/release/examples/misses data/java.sil 1000 noncontiguous 7 40
```

## Next

1. **Phase II tree cache for small corpora.** JS, TS and Go recall runs take 30–80 s because about 1000 candidates are re-parsed per query.
2. **Recommendations are 93% singletons.** Sweep τ2 / τ3 and `ClusterTies` on the quality bench, which now reports cluster sizes.
3. **Corpus hygiene.** Consider excluding generated fixtures (eslint `tests/fixtures/parsers`) or capping method size, as an explicit and documented corpus rule.
4. **Remaining Java non-contiguous misses (48).** They are near-duplicates with +1–4 features from fragments whose nesting was lost (`(x, 0, {#})` shallow parents). A statement-sampled secondary mode would show how much of the gap to the paper's 98.3% is the sampler.
