# Final benchmarks (Stage 6: structural tie-break)

Date: 2026-11-20. Machine: laptop (8 cores, 14 GB RAM). Branch `main`, release build.
Index format **v5** (v4 plus per-method feature counts |F(m)|); every `.sil` was rebuilt for this report.
Raw outputs: `reports/final-raw/`. Stage 5 raw outputs stay in `reports/stage5-raw/`.

Settings: n = 1000 recall queries, bootstrap 1000 (95% CI in brackets), n = 200 quality queries, paper parameters (η1 = 1000, η2 = 100, τ1 = 0.65, τ2 = 1.5, τ3 = 0.9, k = 5).
**Seed 7** is the development seed. The tie-break was chosen on it alone. **Seed 2027** is held out and was run once, after the choice.

## What changed

In Stage 5, Phase II ranked candidates by `(sim DESC, overlap DESC, id ASC)`. When two methods both contained the whole query, `sim` and `overlap` both tied, so rank 1 went to the lower `MethodId`. That capped strict R@1 near 82%.

Stage 6 ranks by `(sim DESC, overlap DESC, full_sim DESC, size ASC, id ASC)` (DESIGN Decision 24):
- `full_sim = |F_full(q) ∩ F(m)|`: overlap with the query's **full** featurization. It includes the cross-line sibling and variable-usage features that query mode drops. Among methods containing the same query-mode features, it prefers the one whose lines are laid out as in the snippet.
- `size = |F(m)|`, smaller first: among equal matches, it prefers the tighter method.
- Both use alpha-renamed features (`#VAR`), never raw identifier names. Ranking is invariant under renaming locals (e2e `sim_ties_break_structurally_and_alpha_invariantly`).
- `sim` and `overlap` still come first, so the new keys only reorder what Stage 5 left to `MethodId`. The containment rate (which depends only on `sim`) is unchanged on both seeds.

Bounded Phase II (`rerank_top`) stays exact. A hit whose `(bound, overlap)` ties the η2-th `(sim, overlap)` can still win on the new keys, so it is no longer skipped blindly. It is loaded only if its optimistic key still beats the η2-th. That key uses an upper bound on `full_sim` computed from the postings (only for such hits) and the `|F(m)|` stored in the v5 index. Proptest `rerank_top_matches_exhaustive` (1,024 cases with heavy ties and none / exact / loose tie bounds) and the e2e bounded-vs-exhaustive test pass.

## Before (Stage 5) vs after (Stage 6): Sillage on Java

| Mode | Seed | Strict R@1 | §5 R@1 | R@100 |
|---|---|---|---|---|
| contiguous | 7 | 82.4% → **85.8%** (+3.4) | 92.9% → **94.0%** (+1.1) | 100.0% → 100.0% |
| contiguous | 2027 (held out) | 81.3% → **84.7%** (+3.4) | 91.5% → **93.4%** (+1.9) | 99.7% → 99.7% |
| non-contiguous | 7 | 79.8% → **84.2%** (+4.4) | 84.5% → **86.9%** (+2.4) | 99.3% → 99.2% (−0.1) |
| non-contiguous | 2027 (held out) | 83.2% → **84.3%** (+1.1) | 86.6% → **87.3%** (+0.7) | 98.6% → 98.8% (+0.2) |

- **It generalizes.** Contiguous queries gain the same +3.4 points on the held-out seed. Non-contiguous queries gain less on 2027 (+1.1) than on 7 (+4.4), but Stage 5 was already 3.4 points higher on 2027.
- **§5 R@1 rises too.** Partial ties (sim < |F(q)|), which §5 never credited, now often put the original first.
- **R@100 moves by at most one or two queries.** Among candidates tied at the depth-100 boundary, a different one now makes the cut.
- Misses (seed 7, `examples/misses.rs`): contiguous 60 §5 misses (Stage 5: 71); non-contiguous 131 (Stage 5: 155).

### Tie-break variants (seed 7 only; `reports/final-raw/tiebreak-sweep-seed7/`)

Each variant extends `sim DESC` with the listed keys and ends with `id ASC`. They were run through a temporary `SILLAGE_TIEBREAK` switch (removed before the commit) with bootstrap 200; point estimates do not depend on the bootstrap count. The shipped row is reproduced exactly by the final binary.

| Keys after `sim` | Strict R@1 contig / non-contig | §5 R@1 contig / non-contig |
|---|---|---|
| overlap (Stage 5) | 82.4% / 79.8% | 92.9% / 84.5% |
| full_sim, overlap | 84.2% / 81.3% | 93.5% / 85.4% |
| overlap, full_sim | 84.1% / 81.3% | 93.4% / 85.4% |
| size, overlap | 85.1% / 84.3% | 93.7% / 86.7% |
| size, full_sim, overlap | 85.1% / 84.2% | 93.7% / 86.6% |
| full_sim, size, overlap | 85.9% / 84.2% | 94.1% / 86.9% |
| **overlap, full_sim, size (shipped)** | **85.8% / 84.2%** | **94.0% / 86.9%** |

- Size does most of the work on non-contiguous queries. `full_sim` adds about 0.7 points on contiguous queries.
- `full_sim, size, overlap` is one query better on contiguous. The shipped order was kept because it only refines the Stage 5 order, and its bounded scan can skip a hit on `(bound, overlap)` alone.

## How to read R@1

- **Strict R@1:** the original method is ranked first. For Sillage, equal-similarity candidates are ordered by Decision 24. BM25 and feature search break ties by method id.
- **§5 R@1:** strict, plus the paper's clause "or the original method body should be 100% similar to the first code snippet in the ranked results" (Decision 9):
  - (a) the rank-1 method has exactly the original's feature multiset (an undeduplicated copy). Applied to **every** system.
  - (b) **Sillage only:** the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them.
  - Partial ties (sim < |F(q)|) earn nothing.
- The pre-Stage 5 rule credited *any* equal-similarity tie ("legacy any-tie", `examples/misses.rs`). It does not depend on tie order, so it is unchanged: 99.2% / 95.5% at seed 7, 98.7% / 96.3% at seed 2027 (contiguous / non-contiguous).

## Java, paper Table 4 (80,192 methods)

`sillage bench recall --index data/java.sil --n 1000 --mode {contiguous,noncontiguous} --bootstrap 1000 --seed {7,2027}`

BM25 and feature-search rows are identical to Stage 5. Only Sillage's Phase II order changed.

| System | Mode | Seed | Strict R@1 | §5 R@1 | R@100 | Paper R@1 / R@100 |
|---|---|---|---|---|---|---|
| BM25 | contig | 7 | 75.8% [73.2, 78.5] | 75.9% [73.1, 78.6] | 99.6% | 78.3% / 96.9% |
| Features (Phase I) | contig | 7 | 81.7% [79.3, 84.1] | 82.2% [79.7, 84.6] | 99.9% | 78.3% / 96.8% |
| **Sillage** | contig | 7 | **85.8%** [83.6, 88.0] | **94.0%** [92.4, 95.5] | **100.0%** | 99.1% / 100% |
| BM25 | contig | 2027 | 73.1% [70.3, 75.8] | 73.3% [70.3, 76.3] | 99.5% | |
| Features (Phase I) | contig | 2027 | 80.5% [78.0, 82.9] | 80.6% [78.2, 83.0] | 99.7% | |
| **Sillage** | contig | 2027 | **84.7%** [82.4, 86.8] | **93.4%** [91.8, 94.8] | **99.7%** | |
| BM25 | non-contig | 7 | 82.9% [80.6, 85.1] | 83.0% [80.4, 85.3] | 99.9% | 93.0% / 99.9% |
| Features (Phase I) | non-contig | 7 | 79.2% [76.8, 81.7] | 79.7% [77.2, 82.0] | 99.3% | 88.1% / 98.6% |
| **Sillage** | non-contig | 7 | **84.2%** [82.0, 86.6] | **86.9%** [84.9, 88.9] | **99.2%** | 98.3% / 100% |
| BM25 | non-contig | 2027 | 82.6% [80.2, 85.0] | 82.6% [80.3, 84.9] | 99.8% | |
| Features (Phase I) | non-contig | 2027 | 82.6% [80.2, 84.9] | 82.6% [80.2, 84.9] | 98.6% | |
| **Sillage** | non-contig | 2027 | **84.3%** [82.0, 86.5] | **87.3%** [85.1, 89.0] | **98.8%** | |

- **Sillage now has the best strict R@1 on every Java row.** It leads feature search by 1.7–5.0 points and BM25 by 1.3–11.6 points. In Stage 5, BM25 led on non-contiguous seed 7 (82.9% vs 79.8%).
- The paper's 99.1% / 98.3% are still not reproduced under the §5 reading. They are matched only by the legacy any-tie rule.

## Multi-language Table 5 (seed 7)

`sillage bench recall --index data/<lang>.sil --n 1000 --mode {contiguous,noncontiguous} --bootstrap 1000 --seed 7`

| Language (methods) | Mode | Sillage strict R@1 (S5 → S6) | Sillage §5 R@1 (S5 → S6) | Sillage R@100 (S5 → S6) | BM25 strict R@1 | Features strict R@1 | Wall / RSS (S6) |
|---|---|---|---|---|---|---|---|
| Python (8,554) | contig | 77.4% → **79.0%** | 78.9% → **80.2%** | 96.7% → 96.6% | 79.1% | 76.9% | 14.6 s / 93 MB |
| Python | non-contig | 83.5% → **85.8%** | 84.3% → **86.5%** | 95.5% → 95.3% | 88.0% | 83.5% | 13.4 s / 93 MB |
| JavaScript (9,347) | contig | 68.1% → **91.1%** | 86.7% → **94.7%** | 99.5% → 99.4% | 81.8% | 67.6% | 72.3 s / 313 MB |
| JavaScript | non-contig | 62.5% → **79.4%** | 67.7% → **80.6%** | 96.6% → 97.1% | 80.6% | 62.7% | 67.5 s / 305 MB |
| TypeScript (5,592) | contig | 73.5% → **77.2%** | 89.1% → **91.5%** | 100.0% → 100.0% | 69.6% | 73.3% | 49.2 s / 163 MB |
| TypeScript | non-contig | 70.4% → **74.5%** | 71.6% → **75.5%** | 98.7% → 98.6% | 78.4% | 70.3% | 48.8 s / 181 MB |
| Go (3,729) | contig | 86.3% → **88.8%** | 91.5% → **93.1%** | 100.0% → 100.0% | 81.0% | 86.1% | 30.4 s / 149 MB |
| Go | non-contig | 84.4% → **85.8%** | 85.4% → **86.9%** | 98.4% → 98.4% | 88.7% | 84.0% | 31.3 s / 145 MB |
| Rust (8,677) | contig | 84.7% → **87.6%** | 96.6% → **97.0%** | 99.4% → 99.4% | 77.1% | 84.3% | 21.8 s / 123 MB |
| Rust | non-contig | 79.6% → **80.3%** | 82.3% → **82.7%** | 97.2% → 97.4% | 80.4% | 79.3% | 22.6 s / 113 MB |
| *Java (80,192)* | contig / non-contig | 82.4% / 79.8% → **85.8% / 84.2%** | 92.9% / 84.5% → **94.0% / 86.9%** | 100.0% / 99.3% → 100.0% / 99.2% | 75.8% / 82.9% | 81.7% / 79.2% | 16.8 / 23.8 s, 222 / 230 MB |

- **Strict R@1 rises on all 12 language-mode cells**, by +0.7 (Rust non-contiguous) to +23.0 (JavaScript contiguous). JavaScript had the most full-query ties: its Stage 5 §5 R@1 was 18.6 points above strict on contiguous queries.
- **Contiguous queries:** Sillage now beats BM25's strict R@1 in every language except Python, where it trails by 0.1 (79.0% vs 79.1%). It beats feature search everywhere.
- **Non-contiguous queries:** BM25 still leads in Python (+2.2), JavaScript (+1.2), TypeScript (+3.9), Go (+2.9) and Rust (+0.1). The gap was 0.8–18.1 points in Stage 5.
- **R@100** moves by −0.2 to +0.5 points, for the depth-boundary reason above.
- JS / TS / Go recall runs remain slow: the bounded Phase II cannot prune on small corpora (Stage 3 known limit). Wall times were taken while other jobs loaded the laptop (load average about 5), so compare them only roughly with Stage 5.

## Recommendation quality (Java, n = 200, leave-one-out)

`sillage bench quality --index data/java.sil --n 200 --seed {7,2027}`

| Metric | Stage 5 seed 7 | **Stage 6 seed 7** | Stage 5 seed 2027 | **Stage 6 seed 2027** |
|---|---|---|---|---|
| Containment rate (query-mode F(q), ≥ τ1) | 73.0% | **73.0%** | 74.0% | **74.0%** |
| Avg precision of added features | 46.5% | 46.9% | 46.4% | 46.6% |
| Avg recall of held-out features | 64.6% | 64.2% | 67.0% | 67.3% |
| Avg recommendations / query | 2.65 | 2.65 | 2.79 | 2.79 |
| Avg extra lines / recommendation | 26.5 | 25.5 | 25.5 | 25.2 |
| Exact-match rate (added lines ⊆ held-out) | 3.0% | 3.0% | 0.5% | 0.5% |
| Line precision / recall (best rec) | 37.9% / 48.0% | 37.4% / 46.8% | 35.7% / 46.5% | 35.8% / 46.6% |
| Queries with a multi-method cluster | 16.5% | 16.5% | 23.5% | 24.5% |
| Cluster sizes (1 / 2 / 3–5 / 6+) | 489 / 30 / 9 / 2 | 490 / 29 / 7 / 4 | 495 / 48 / 14 / 2 | 491 / 53 / 12 / 3 |

Containment and recommendation counts are identical, since they depend only on `sim`. Ties now feed tighter methods into Phase III, so recommendations add about one line fewer. The other metrics move by ≤ 1.2 points, within n = 200 noise.

## Latency and indexing (Java, 80k methods)

Latency was measured with the Stage 5 binary (commit `3f0f6ca`, v4 index) and the Stage 6 binary alternating in one session, three rounds per seed (`reports/final-raw/latency-paired.txt`). The laptop had a load average of about 5 from other jobs, so absolute numbers are a little higher than in Stage 5's report. The third seed-2027 round was disturbed for both binaries and is left out of the ranges below.

| `recommend` (quality bench, 200 queries) | Stage 5 binary | **Stage 6 binary** |
|---|---|---|
| p50, seed 7 / 2027 | 2.7–2.8 / 3.1 ms | **2.8–2.9 / 3.0–3.1 ms** |
| p95, seed 7 / 2027 | 19.7–20.6 / 25.4–26.4 ms | 22.9–23.6 / 27.1–29.0 ms |
| max, seed 7 / 2027 | 32–41 / 41–44 ms | 33–39 / 43–46 ms |
| Phase II p95, seed 7 | 14.1–14.8 ms | 17.5–18.3 ms |
| Candidates re-parsed, seed 7 (sum over 200 queries) | 3,154 | 3,306 (+4.8%) |

- **p50 and max are unchanged; p95 rises by 1–3.5 ms.** The extra cost is all in the 11 of 200 queries that fill η2 = 100 above τ1. There, hits tied with the 100th candidate must be re-parsed when they could win the tie-break: +1 to +25 parses per query.
- A first version without the v5 size bound and the posting-based `full_sim` bound re-parsed every tied hit: p95 27 ms, max 135 ms. Loading tied hits in smaller chunks cut parses to 3,266 but not latency (less parallelism), so it was dropped.
- `sillage index --lang java`: 6.4–6.8 s (v5; Stage 5: 6.5 s v4), same 80,192 methods / 883,117 features / 5,288,669 postings. The per-method sizes add 320 KB.

## Stage 5: the 17 review findings

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

`cargo test --workspace --release`: **163 passed, 0 failed, 1 ignored** (the ignored test is the on-corpus parser check, run by hand; Stage 5: 157), 0 warnings. `cargo build --release --bins --examples`: 0 warnings.

New in Stage 6:
- `rerank::tests::ties_prefer_full_query_match_then_tighter_method`
- `rerank::tests::rerank_top_loads_hits_tied_at_the_kth_similarity`
- the proptests `tie_fields_match_definitions`, `rerank_top_matches_exhaustive` (now with tie bounds and a full query different from F(q)) and `tie_break_preserves_sim_order`
- `index::tests::sizes_and_resolved_bounds`
- `e2e::sim_ties_break_structurally_and_alpha_invariantly`; `e2e::single_pass_index_and_bounded_rerank_are_exact` now also compares `full_sim` and `size`

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
