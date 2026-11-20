# Stage 5: final benchmarks after the adversarial review fixes

Date: 2026-11-20. Machine: laptop (8 cores, 14 GB RAM). Branch `main`, release build.
Index format **v4** (xxh3 payload checksum, validated on load); every `.sil` was rebuilt for this report.
Raw outputs: `reports/stage5-raw/`.

Settings: n = 1000 recall queries, bootstrap 1000 (95% CI in brackets), n = 200 quality queries, paper parameters (η1 = 1000, η2 = 100, τ1 = 0.65, τ2 = 1.5, τ3 = 0.9, k = 5).
**Seed 7** is the development seed used in Stages 2 and 3. **Seed 2027** is a fresh held-out seed that no change was tuned on.

## How to read R@1

- **Strict R@1:** the original method is ranked first. Ties are broken by method id. This is the like-for-like column across systems.
- **§5 R@1:** strict, plus the paper's clause "or the original method body should be 100% similar to the first code snippet in the ranked results" (Decision 9):
  - (a) the rank-1 method has exactly the original's feature multiset (an undeduplicated copy). Applied to **every** system.
  - (b) **Sillage only:** the original ties rank 1 at the full query similarity |F(q)|. Both methods then contain the whole query, so the query cannot separate them. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them.
  - Partial ties (sim < |F(q)|) earn nothing.
- Up to Stage 3, *any* equal-similarity tie at rank 1 counted, and only for Sillage. That rule gave the Stage 3 figure of 99.1%. On today's binary and query sets the old rule ("legacy any-tie", `examples/misses.rs`) gives:

| Java Sillage R@1 | strict | **§5** | legacy any-tie (Stage 3 rule) |
|---|---|---|---|
| contig, seed 7 | 82.4% | **92.9%** | 99.2% |
| non-contig, seed 7 | 79.8% | **84.5%** | 95.5% |
| contig, seed 2027 | 81.3% | **91.5%** | 98.7% |
| non-contig, seed 2027 | 83.2% | **86.6%** | 96.3% |

So the drop from Stage 3's 99.1% / 95.2% comes from **scoring**, not from a regression. Under the old rule, seed 7 gives 99.2% / 95.5%, the same as Stage 3 within noise. That stays true even though removing the Java sampler exemption changed the Java query sets (fingerprints `1f8ace98b02e5c27` / `51f282b217bd929b`, Stage 3: `cdb10bf0fdb864a0` / `b1aabba2562f9058`).

Roughly 10 points of contiguous R@1 are full-query ties. The 5-line prefix occurs whole in the original and in another method (test suites, overloads). Another ~6 points were partial ties, which the old rule credited and which are now misses.

## Java, paper Table 4 (80,192 methods)

`sillage bench recall --index data/java.sil --n 1000 --mode {contiguous,noncontiguous} --bootstrap 1000 --seed {7,2027}`

| System | Mode | Seed | Strict R@1 | §5 R@1 | R@100 | Paper R@1 / R@100 |
|---|---|---|---|---|---|---|
| BM25 | contig | 7 | 75.8% [73.2, 78.5] | 75.9% [73.1, 78.6] | 99.6% | 78.3% / 96.9% |
| Features (Phase I) | contig | 7 | 81.7% [79.3, 84.1] | 82.2% [79.7, 84.6] | 99.9% | 78.3% / 96.8% |
| **Sillage** | contig | 7 | **82.4%** [79.9, 84.7] | **92.9%** [91.1, 94.5] | **100.0%** | 99.1% / 100% |
| BM25 | contig | 2027 | 73.1% [70.3, 75.8] | 73.3% [70.3, 76.3] | 99.5% | |
| Features (Phase I) | contig | 2027 | 80.5% [78.0, 82.9] | 80.6% [78.2, 83.0] | 99.7% | |
| **Sillage** | contig | 2027 | **81.3%** [78.9, 83.6] | **91.5%** [89.8, 93.0] | **99.7%** | |
| BM25 | non-contig | 7 | 82.9% [80.6, 85.1] | 83.0% [80.4, 85.3] | 99.9% | 93.0% / 99.9% |
| Features (Phase I) | non-contig | 7 | 79.2% [76.8, 81.7] | 79.7% [77.2, 82.0] | 99.3% | 88.1% / 98.6% |
| **Sillage** | non-contig | 7 | **79.8%** [77.4, 82.2] | **84.5%** [82.1, 86.6] | **99.3%** | 98.3% / 100% |
| BM25 | non-contig | 2027 | 82.6% [80.2, 85.0] | 82.6% [80.3, 84.9] | 99.8% | |
| Features (Phase I) | non-contig | 2027 | 82.6% [80.2, 84.9] | 82.6% [80.2, 84.9] | 98.6% | |
| **Sillage** | non-contig | 2027 | **83.2%** [80.9, 85.5] | **86.6%** [84.6, 88.5] | **98.6%** | |

- **The held-out seed agrees with the development seed.** Every Sillage cell for 2027 lies within the seed-7 CI or within 1.5 points of it. Non-contiguous strict R@1 is higher on 2027 (83.2% vs 79.8%).
- **Strict R@1:** Sillage beats feature search on every row, by only 0.6–0.8 points, because they share F(q) and tie in the same places. BM25 wins on non-contiguous strict R@1 for seed 7 (82.9% vs 79.8%).
- **§5 R@1:** Sillage leads by 10.7–18.2 points on contiguous queries and by 1.5–4.0 points on non-contiguous ones. Most of that lead comes from the full-query-tie clause, which only Sillage gets.
- **R@100:** Sillage is at 98.6–100%.
- The paper's 99.1% / 98.3% are not reproduced under the stricter §5 reading. They are matched only under the legacy any-tie rule (99.2% / 95.5%).

Misses (seed 7, from `examples/misses.rs`): contiguous 71 §5 misses (29 at rank 2, 26 at ranks 3–5, 0 absent); non-contiguous 155 (74 at rank 2, 7 absent).

## Multi-language Table 5 (seed 7)

`sillage bench recall --index data/<lang>.sil --n 1000 --mode {contiguous,noncontiguous} --bootstrap 1000 --seed 7`

| Language (methods) | Mode | Sillage strict R@1 | **Sillage §5 R@1** | Sillage R@100 | BM25 strict R@1 | Features strict R@1 | Wall / RSS |
|---|---|---|---|---|---|---|---|
| Python (8,554) | contig | 77.4% | **78.9%** | 96.7% | 79.1% | 76.9% | 12.3 s / 84 MB |
| Python | non-contig | 83.5% | **84.3%** | 95.5% | 88.0% | 83.5% | 13.1 s / 98 MB |
| JavaScript (9,347) | contig | 68.1% | **86.7%** | 99.5% | 81.8% | 67.6% | 73.3 s / 330 MB |
| JavaScript | non-contig | 62.5% | **67.7%** | 96.6% | 80.6% | 62.7% | 60.3 s / 289 MB |
| TypeScript (5,592) | contig | 73.5% | **89.1%** | 100.0% | 69.6% | 73.3% | 43.7 s / 195 MB |
| TypeScript | non-contig | 70.4% | **71.6%** | 98.7% | 78.4% | 70.3% | 53.3 s / 183 MB |
| Go (3,729) | contig | 86.3% | **91.5%** | 100.0% | 81.0% | 86.1% | 30.4 s / 125 MB |
| Go | non-contig | 84.4% | **85.4%** | 98.4% | 88.7% | 84.0% | 29.3 s / 137 MB |
| Rust (8,677) | contig | 84.7% | **96.6%** | 99.4% | 77.1% | 84.3% | 15.8 s / 107 MB |
| Rust | non-contig | 79.6% | **82.3%** | 97.2% | 80.4% | 79.3% | 16.3 s / 101 MB |
| *Java (80,192)* | contig / non-contig | 82.4% / 79.8% | **92.9% / 84.5%** | 100.0% / 99.3% | 75.8% / 82.9% | 81.7% / 79.2% | 16.8 / 21.3 s, 227 / 233 MB |

- R@100 equals Stage 3 on every language, since retrieval did not change. The Stage 3 R@1 column (92.8% Python contiguous, etc.) used the legacy any-tie rule and is not comparable to either R@1 column here.
- On non-contiguous queries BM25 beats Sillage's strict R@1 in every language. On contiguous queries Sillage's strict R@1 is ahead in Go, TS and Rust and behind in Python and JS.
- JS / TS / Go recall runs remain slow (30–73 s): the bounded Phase II cannot prune on small corpora (Stage 3 known limit).
- 17 TypeScript source files are not valid UTF-8. `sillage index` now reports them as a warning instead of skipping them silently; the method count, 5,592, is unchanged.

## Recommendation quality (Java, n = 200, corrected leave-one-out)

`sillage bench quality --index data/java.sil --n 200 --seed {7,2027}`

The query's own method is now excluded **before** Phase II (`Engine::recommend_excluding`). Previously the bench filtered it out of the final recommendations, after it had already taken an N2 slot, joined clusters and won Jaccard dedup. Containment now uses the query-mode F(q) the engine matches. Added-feature precision / recall keep the full featurization on both sides.

| Metric | Stage 3 (seed 7, old protocol) | **Stage 5 seed 7** | **Stage 5 seed 2027** |
|---|---|---|---|
| Containment rate (query-mode F(q), ≥ τ1) | 70.0% | **73.0%** | **74.0%** |
| Avg precision of added features | 48.2% | 46.5% | 46.4% |
| Avg recall of held-out features | 67.2% | 64.6% | 67.0% |
| Avg recommendations / query | 2.29 | **2.65** | **2.79** |
| Avg extra lines / recommendation | 24.9 | 26.5 | 25.5 |
| Exact-match rate (added lines ⊆ held-out) | 3.5% | 3.0% | 0.5% |
| Line precision / recall (best rec) | 40.9% / 51.7% | 37.9% / 48.0% | 35.7% / 46.5% |
| Queries with a multi-method cluster | 13.5% | **16.5%** | **23.5%** |
| Cluster sizes (1 / 2 / 3–5 / 6+) | 427 / 23 / 7 / 1 | 489 / 30 / 9 / 2 | 495 / 48 / 14 / 2 |

The original no longer takes a slot, so queries get more recommendations and more multi-method clusters. Precision and line metrics drop by 1–3 points, about the n = 200 noise. Most recommendations are still full-body singletons (92% at seed 7).

## Latency and indexing (Java, 80k methods)

| | Stage 3 | **Stage 5** |
|---|---|---|
| `sillage index --lang java` | 7.1 s, 605 MB (v3) | **6.5 s, 603 MB** (v4, same 80,192 methods / 883,117 features / 5,288,669 postings) |
| Recall bench contig / non-contig (wall, seed 7) | 16.3 / 15.9 s | 16.8 / 21.3 s (the §5 duplicate check re-parses up to 3 rank-1 methods per query; not profiled) |
| Quality bench (200 queries) | 3.2 s | 4.2 s |
| `recommend` p50 / p95 / max | 2.7 / 17.6 / 43.2 ms | 3.4 / 23.3 / 43.6 ms |
| Phase I p50 / p95 | 0.8 / 1.2 ms | 1.4 / 2.0 ms (likely the per-candidate language / exclusion filter; not profiled) |
| Phase II p50 / p95 | 1.1 / 12.7 ms | 1.1 / 17.8 ms |
| Phase III p50 / p95 | 0.4 / 3.8 ms | 0.4 / 4.7 ms |

Other languages index in 0.6–2.0 s at 75–228 MB. The cost of the load-time checksum and validation was not measured separately.

## The 17 review findings

| # | Severity | Fix | Regression test |
|---|---|---|---|
| 1 | High | Format v4: `xxh3_64` payload checksum checked before decoding; older versions rejected with a rebuild hint. `Corpus::validate` (ids positional, body spans in bounds on char boundaries), `InvertedIndex::validate` (CSR offsets, counts, posting range, `num_methods`), `Bm25::validate` | `bm25_store::{load_rejects_flipped_bytes_without_panicking, corrupted_payload_with_valid_checksum_never_panics, load_validates_structures}`, `index::tests::validate_catches_inconsistent_csr`, `bm25::validate_tests` |
| 2 | High | Phase I (and BM25 / feature search) filter by `lang_compatible` (TS = TSX family) **before** the η1 cut | `e2e::mixed_language_index_stays_in_language` (fails without the fix) |
| 3 | High | §5 R@1 only for full-query ties (Sillage) and feature duplicates (all systems); strict R@1 reported in Markdown and JSON | `recall::tests::{decision_9_duplicate_clause, decision_9_full_query_tie}`, `report::tests` |
| 4 | High | `Engine::recommend_excluding`; the quality bench excludes the original before Phase II | `e2e::recommend_excluding_drops_methods_before_phase_two` |
| 5 | Medium | Containment uses `featurize_query`; added features use full `featurize` (documented in code and report) | `quality::tests` |
| 6 | Medium | `search --format json` emits one document `{lang, results: [{rank, id, repo, path, name, line_start, score, sim, overlap, render?}]}` with `locate` and `--max-lines` | `cli::cli_stage5_regressions` |
| 7 | Medium | `index` fails on a missing / unreadable root and on 0 methods; walk / read / oversize counts are reported on stderr | `cli::cli_stage5_regressions` |
| 8 | Medium | Multi-line string interiors skipped for every language, Java included | `sample::tests::code_lines_skip_multiline_string_interiors` |
| 9 | Medium | v4 indexes rebuilt; stale `data/stage2-*.sil`, `data/java-stage2.sil` deleted; seeds 7 and 2027 run | this report |
| 10 | Medium | MIT `LICENSE`; README rewritten (commands, fixtures, flags, tables) | — |
| 11 | Medium | DESIGN.md: Decisions 3 / 9 / 13 reconciled, Decisions 18–23 (query mode, holes, bounded Phase II, language families, leave-one-out, BM25 case), index format v4, `sillage-bench → sillage-render` edge | — |
| 12 | Low | `u32` overlap / `u64` bound accumulators; checked `u32` conversions for CSR offsets, BM25 offsets / doc ids / lengths, corpus ids | `index::tests::wide_overlaps_and_filtered_search` (70,000 shared features) |
| 13 | Low | Scanner: `\` escapes in single-quoted strings and char literals; Rust lifetimes are not literals; Java `"""` text blocks are one literal | `snippet::tests::{scan_handles_single_quote_escapes, scan_treats_java_text_blocks_as_one_literal}` |
| 14 | Low | Cluster growth in frontier chunks stops at exactly `MAX_TUPLES`; Jaccard dedup prefilter (size ratio, range overlap) | `toggles::many_identical_candidates_do_not_explode` (asserts exactly 5,000), `cluster::tests::may_exceed_is_a_sound_prefilter` |
| 15 | Low | Dead `let _ = nodes;` fixture and the `has_counts == false` branch removed | — |
| 16 | Low | `--lang` resolved before reading stdin; `resolve_lang` uses `lang_from_path` (`.pyi`, `.mts`, …); `--k/--eta1/--eta2/--max-lines ≥ 1`, `--tau1/--tau3 ∈ [0,1]`, `--tau2` finite ≥ 0; ANSI output escapes control bytes; file-line gutter in ANSI and `data-line` in HTML | `cli::cli_stage5_regressions`, `render::tests::{ansi_escapes_control_characters, located_layout_shows_file_lines}` |
| 17 | Low | `stage3-improvements.md` line 110 clarified (isolated ablation vs full table); this report | — |

## Tests

`cargo test --workspace --release`: **157 passed, 0 failed, 1 ignored** (the ignored test is the on-corpus parser check, run by hand), 0 warnings. `cargo build --release --bins --examples`: 0 warnings.

## Reproduce

```sh
cd /home/george/Projects/sillage
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR=$PWD/target TMPDIR=$PWD/target/tmp CARGO_BUILD_JOBS=4; mkdir -p $TMPDIR
cargo build --release --bins --examples
for l in java python javascript typescript go rust; do target/release/sillage index --corpus data/corpus --out data/$l.sil --lang $l; done
for s in 7 2027; do for m in contiguous noncontiguous; do
  target/release/sillage bench recall --index data/java.sil --n 1000 --mode $m --bootstrap 1000 --seed $s; done
  target/release/sillage bench quality --index data/java.sil --n 200 --seed $s; done
for l in python javascript typescript go rust; do for m in contiguous noncontiguous; do
  target/release/sillage bench recall --index data/$l.sil --n 1000 --mode $m --bootstrap 1000 --seed 7; done; done
target/release/examples/misses data/java.sil 1000 contiguous 7 0   # strict / §5 / legacy any-tie
```
