# Stage 2: integration, end-to-end paper check, real-corpus benchmark (v1)

Date: 2026-11-20. Machine: laptop (8 cores, 14 GB RAM). Branch `feat/integration-stage2`.

## Summary

- The real pipeline (parse → index → engine → render → bench) reproduces **Listing 7 exactly** from Listings 4–6, and the real tree-sitter parse of Listing 4 matches **Table 2** for both `view₃₀` and `0₂₀`. Table 2 prints `#;#;#;`; we produce the Fig. 4 shape `#;#;#`.
- I fixed **15 cross-crate or algorithm bugs** found by running real code. Four of them each moved recall by several points or removed a hang.
- On **80,192 real Java methods** (10 repos), **contiguous recall matches the paper**: Sillage R@1 **98.9%** against Aroma's 99.1%, and R@100 **99.9%** against 100%.
- **Non-contiguous recall still trails** the paper: R@1 **91.5%** against 98.3%. The cause is analysed below.
- `cargo test --workspace`: **125 passed, 0 failed, 0 ignored** across 26 test binaries, with 0 warnings. `cargo build --release` also has 0 warnings.

## Corpus and index

| | |
|---|---|
| Repos (Java, pinned SHAs in `corpus.toml`) | jsoup, junit4, commons-io, commons-lang, retrofit, guava, ExoPlayer r2.19.1, jackson-databind 2.17.2, gson 2.11.0, javapoet 1.13.0 (the last four were added this stage) |
| Fetch (`sillage fetch`, all 18 manifest repos) | 28 s cold; 15 s with 4 more repos added |
| Java files / source size | 8,265 files / 87 MB |
| Methods after project/file/method dedup | **80,192** |
| Distinct features / postings | 883,117 / 5,288,669 |
| Index time (`sillage index --lang java`) | **10.6 s** wall (8 threads), peak RSS 572 MB |
| Index file `data/java.sil` | **83.5 MB** |

At 45k methods (the first 6 repos) indexing took 4.9 s and the index was 44.8 MB.

## Recall: paper Table 4 (n = 1000, bootstrap 1000, seed 7)

`sillage bench recall --index data/java.sil --n 1000 --mode {contiguous,noncontiguous} --bootstrap 1000 --seed 7`

Contiguous: 154 s wall, about 0.15 s per query for all three systems.

| System | Ours R@1 [95% CI] | Paper R@1 | Ours R@100 | Paper R@100 |
|---|---|---|---|---|
| BM25 keywords | 73.2% [70.3, 76.0] | 78.3% | 99.7% | 96.9% |
| Features search (Phase I) | 83.4% [81.1, 85.7] | 78.3% | 99.9% | 96.8% |
| **Sillage (Phases I+II)** | **98.9% [98.2, 99.5]** | **99.1%** | **99.9%** [99.7, 100] | **100%** |

Non-contiguous: 167 s wall.

| System | Ours R@1 [95% CI] | Paper R@1 | Ours R@100 | Paper R@100 |
|---|---|---|---|---|
| BM25 keywords | 81.0% [78.6, 83.4] | 93.0% | 99.8% | 99.9% |
| Features search | 80.2% [77.8, 82.6] | 88.1% | 99.3% | 98.6% |
| **Sillage** | **91.5% [89.8, 93.2]** | **98.3%** | **99.3%** [98.8, 99.8] | **100%** |

### Ablation and progression (n = 200, seed 7, `examples/ablation.rs`)

| State | Mode | Features R@1 | Sillage R@1 | Sillage R@100 |
|---|---|---|---|---|
| Slices as delivered (45k corpus, n = 100) | contig | 84.0% | 97.0% | 99.0% |
| + body-only trees, singletons, for-shape (80k corpus) | contig | 73.0% | 86.5% | 95.0% |
| + symmetric free-name scoping (bug 6) | contig | 75.0% | 93.0% | 97.0% |
| + code-line sampling (bug 7) + auto-closers (bug 8) | contig | 80.5% | **98.5%** | 99.5% |
| same, `PaperFeaturizer` (no sequence normalisation) | contig | 81.0% | 96.5% | 99.5% |
| final, default featurizer | noncontig | 76.0% | 87.5% | 99.0% |
| final, `PaperFeaturizer` | noncontig | 75.5% | 86.5% | 99.0% |

Statement-sequence normalisation (bug 9) adds **+2.0 points** contiguous and **+1.0** non-contiguous Sillage R@1. The large drop from 97% to 86.5% came from moving to the larger, more repetitive 80k corpus (ExoPlayer and guava test suites), which exposed bugs 6–8.

## Recommendation quality (§4.2-style, half-snippet queries, n = 200, seed 7)

`sillage bench quality --index data/java.sil --n 200 --seed 7` took 37 s wall; latency is measured one query at a time.

| Metric | Value |
|---|---|
| Containment rate (some recommendation has ≥ τ1 of F(q)) | 70.5% |
| Avg precision of added features (best recommendation per query, by F1) | 49.5% |
| Avg recall of held-out second-half features | 67.2% |
| Avg recommendations per query (leave-one-out) | 2.10 |
| Avg extra lines per recommendation | 25.8 |

Latency from the clean background run (no other load):

| Phase | p50 | p95 | max |
|---|---|---|---|
| parse | 0.2 ms | 1.7 ms | 3.8 ms |
| Phase I (index search + re-parse of 1000 candidates) | 132.1 ms | 187.1 ms | 211.3 ms |
| Phase II (prune + rerank) | 14.1 ms | 23.6 ms | 28.0 ms |
| Phase III (cluster + intersect) | 0.5 ms | 3.7 ms | 9.0 ms |
| **End to end** | **147.2 ms** | **210.2 ms** | 237.5 ms |

The paper reports 2.9 s on a 4-core desktop and 1.6 s on a 24-core server, both on a 2.4M-method corpus. Our corpus is about 30× smaller, and Phase I scales linearly with postings. Before bug 5 the max was **42,491 ms**; p95 was 133 ms and hid it.

The paper's §4.2 metric ("one of the top 5 recommendations matched the original snippet": 37/50 = 74%) was judged by hand. Our automated containment and feature precision/recall are a proxy and are not directly comparable.

## Real queries (`sillage query`, 80k index)

The end-to-end CLI wall time is about 0.22–0.29 s per query, including the index load. The engine itself takes 87–138 ms.

| Query | Result |
|---|---|
| `viewgroup_loop.java` (Listing 4) | One singleton: an ExoPlayer `getChildAt` loop over `extraControls`. The corpus has no `setupUI` pattern (Android-app corpus needed). |
| `bitmap_decode.java`, `underline_text.java` | No recommendation: no corpus method passes τ1 = 0.65 (top score 0.72 has no cluster; no Android-app code). |
| `package_version.java` | One singleton. |
| `equals.java` (`this == obj` / `instanceof`) | **Cluster of 4** (commons-io and 3 ExoPlayer classes), l = 3.45, s = 0.92. Recommends the `final X other = (X) obj; return Objects.equals(…) && …` continuation. Good. |
| `checknotnull.java` | Cluster of 6 heterogeneous methods (ExoPlayer activity + 5 guava). The intersection is dominated by the first member's long `checkNotNull` chain. Weak. |
| `jsoup_select.java`, `read_lines.java`, `char_loop.java`, `map_entries.java` | Singletons only (full bodies). They are relevant, but they are search results, not synthesized patterns. |

## Bugs fixed during integration

Each bug below was found by running the real pipeline and has a regression test.

1. **Method signatures leaked into recommendations.** Corpus trees were the whole method, so prune kept parameter tokens and the output started `public void (View view) {`. `parse_method` now parses the whole method (so parameters still scope as locals) but returns the **body subtree** (`sillage-parse/src/snippet.rs`, `body_subtree`). Recommendations and features are body-only, as in the paper.
2. **`for` loop shape differed from Fig. 4 / Table 2, and between loops.** The loop read `for(##;#)#` with a declaration initialiser `int#;`, but `for(#;#;#)#` with an expression initialiser. The lowering now groups the header into `#;#;#` and hoists the declaration's `;` (`lower.rs`, `group_for_control`). Real Listing 4 now gives exactly `(0, 1, int#)` and `(0, 1, #;#;#)`.
3. **Size-1 clusters recommended the query back.** `Intersect((i1), q) = Prune(F(q), N2(i1))` only keeps query code, so the output was the query plus `// ...` placeholders. §4.3.5 says singletons recommend "the full method bodies without any pruning". This is now `Params::singleton = Singleton::FullBody`, with the literal formula kept as a toggle.
4. **Recommendations rendered the outer body braces mis-indented** (`{` … `    }`). The layout now omits the root block's `{`/`}`. Closing-brace lines of a recommended block were also not highlighted. Keyword-only lines now inherit "extra" when their enclosing node has no query-matched token, which matches Listing 7's highlighting.
5. **Exponential Phase III (42 s on one query).** The literal Decision 3 rule ("ties add every maximiser") enumerates every increasing sequence of tied candidates. That blows up on retrofit's `RequestFactoryTest` and other runs of identical test methods. The default is now `ClusterTies::First`; `All` is kept behind a `MAX_TUPLES = 5000` cap. Max Phase III dropped from 42,444 ms to 9 ms with identical quality metrics.
6. **Asymmetric scoping of free names (the biggest recall bug).** Decision 8 made free identifiers `#VAR` in snippets but globals in methods. Every field (`player`, `_type`, …) mismatched across query and corpus, and the original method could fall out of the top 400. Free value identifiers are now locals in both. Contiguous Sillage R@1 rose from 86.5% to 93.0%.
7. **The §5 sampler counted comment lines as code.** Some queries were five comment lines (zero features). Queries are now built from **code lines**: lines touched by a terminal of the parsed body (`sample.rs`, `code_lines`). This also fixes Python's first-line indentation in queries.
8. **Partial snippets lost their closing keywords.** `if (x) {` with no `}` was closed by the wrapper's brace, which was then dropped, giving the label `{##` instead of `{##}`. Tree-sitter `MISSING` tokens were dropped the same way. Missing `)`/`]`/`}` closers are now appended and kept as **zero-width keywords**, and so are `MISSING` tokens (`snippet.rs`, `closers`; `lower.rs`, `phantom`). Together with bug 7 this raised contiguous R@1 from 93.0% to 98.5%.
9. **Multi-statement queries never shared shallow parent features with the corpus.** Query statements hang off a root `###` (slots 1..k), while in the method they hang off `{#######}` at other slots. `AromaFeaturizer` now emits statement-sequence parents and contexts as `(n, 0, {#})`. `PaperFeaturizer` keeps the literal labels for ablation. Worth +2.0 points R@1.
10. **Keyword-only subtrees vanished under pruning.** `result = {}` rendered as `result =` (also `[]`, `()`). `SimplifiedTree::retain` now keeps token-less subtrees of kept nodes. Features are unchanged.
11. **Template strings and f-strings were atomic.** `` `HTTP ${res.status}` `` hid a renamable local, so JS functions that differed only by variable names never clustered. Interpolated strings are now lowered structurally (`has_interpolation`), and Python `string_start`/`string_end` are keywords.
12. **Intersections had syntactic holes.** Examples: `(innerView);` (dropped callee, Listing 7), `instanceof)`, `() obj`, `return.equals(`, `inputPlayerView.;`. A presentation pass (`Params::complete_calls`, `engine/src/complete.rs`) restores, from the first cluster member, dropped children of up to 3 tokens that sit next to a non-delimiter keyword of a kept node. Quality metrics are unchanged (precision 49.6% → 49.5%). Listing 7 now matches the paper exactly, including `setupUIToHideKeyBoardOnTouch(innerView);`.
13. **Comments rendered as `// ...` placeholders** in full-body recommendations, because any non-blank gap counted as dropped code. Comment-only gaps are now copied verbatim (`gap_has_code`). A dropped `*deref` line still gets a placeholder.
14. **The quality benchmark hid outliers.** It now reports max latency and per-phase p50/p95/max (`QualityReport::{max_ms, phases}`). `SILLAGE_TRACE=1` prints per-query timings.
15. **Fixture tests encoded the old behaviour.** Tests asserted signature tokens in method trees and method-mode globals for fields. They now use a `signature_only` guard, assert locals for fields, and check that every node of an extracted tree lies inside the body span.

### Tests added

- **`crates/sillage-cli/tests/e2e.rs`** (7 tests, no hand-built trees):
  - real Listing 4 against Table 2;
  - uniform `for` shapes in Java, JS and TS;
  - snippet and method-body features of Listing 4 are **identical**;
  - Listing 4 → Listing 7 through walk, index, BM25, save/load and recommend, checked in all 4 renderers with exact text and highlighting;
  - persistence round-trip equality;
  - all **7 languages** end to end with real mini-corpora (cluster found, shared extension kept, per-function line dropped, placeholder `...` for Python and `// ...` elsewhere, query lines not highlighted);
  - comment rendering.
- **`crates/sillage-cli/tests/cli.rs`**: drives the real binary through `index`, `stats`, `explain` (file and stdin), `search` (with `--show`), `query` in plain/ansi/html/json and with tuning flags, `bench recall` (both modes, markdown and JSON), `bench quality` (markdown and JSON), and error paths.
- Engine: `many_identical_candidates_do_not_explode`, plus tie-policy and singleton-mode tests.
- Bench: `code_lines_skip_comments_and_braces`.

## Remaining weaknesses, ranked for the next pass

### Recall

1. **Non-contiguous R@1 is 91.5% against the paper's 98.3%.** Of 25 misses in n = 200, almost all have the original at **rank 2–7**, with a sibling method (same test class or overload) **1–6 features higher**. The queries are random *physical* lines, so multi-line statements are cut into fragments (`.build();`, `assertEquals(`, `        .where(`). These parse into ERROR trees, and greedy prune matches the fragments to a near-duplicate.
   - Options:
     - (a) Sample whole statements or logical lines instead of physical lines. The paper's ANTLR-era "lines" were likely closer to this.
     - (b) Exact or beam prune for small queries.
     - (c) Tie-break on Phase I overlap or on cosine over the full method.
     - (d) Strip dangling `,`, `.` and operators before adding closers.
   - Our corpus (ExoPlayer and guava test suites) is far more self-similar than the paper's deduplicated Android apps.
2. **Contiguous residual misses** fall into three cases:
   - a statement cut inside an argument list with a trailing `,` (`this(uri, …, 0,`);
   - `this(…)`/`super(…)` that only parse inside a constructor wrapper (add a constructor wrapper candidate);
   - near-duplicates (the paper's Listing 8 case).
3. **Decision 9 counts only exact ties.** A near-duplicate with sim + 1 is a miss. The paper's "100% similar" may have been looser. Worth reporting a "≥ 99% similar" variant.
4. **BM25 R@1 (73.2%) is below the paper's (78.3%).** The tokenizer adds camelCase sub-words, which may dilute IDF. Try whole identifiers only, or weight them.

### Recommendation quality

5. **Most recommendations are singletons**, i.e. full method bodies (avg 25.8 extra lines). τ2 = 1.5 / τ3 = 0.9 rarely form clusters on this corpus. Clusters appeared only for common idioms (`equals`, `checkNotNull`) in the real queries. Possible causes: corpus size (80k against 2.4M), and the `PrunedFirst` s-denominator interacting with `ClusterTies::First`.
   - Next: sweep τ2/τ3 and `ClusterTies` on the quality bench, and report the cluster-size distribution.
6. **Heterogeneous clusters intersect poorly.** The `checkNotNull` cluster joined 6 unrelated methods through generic `checkNotNull(#VAR)` features; the result is the first member's long chain. Options: an `l` cap, a minimum `cs` of non-`#VAR` features, or requiring cluster members to share callee or type tokens.
7. **Greedy prune mis-attributes `#VAR` leaves.** In Listing 5 it can match a query `view` to `view.setOnTouchListener`, which un-highlights the `});`/`}` lines. A majority rule now hides this in rendering, but `query_match` is still imperfect. An alignment-aware tie-break (prefer leaves whose var-usage features also match) would fix it.
8. **The quality metric is a feature-level proxy**, not the paper's manual "exact recommendation". Add an automatic exact-match check: the held-out lines appear, normalized, in the rendered recommendation.

### Rendering and CLI

9. **The JSON `src_line` is relative to the method text**, not the file. Add `line_start` per cluster member so editors can jump to it.
10. A leading comment before the first kept terminal is not shown. Recommendations can be long; there is no `--max-lines`.
11. `complete_calls` only fills holes of up to 3 tokens. Larger holes, such as a dropped lambda body inside a kept call, still render oddly (`view.` when a whole regrouped call is dropped).

### Performance

12. **Phase I dominates latency (~90% of 147 ms).** It re-parses all η1 = 1000 candidates per query with tree-sitter. Options: store postcard-serialized `SimplifiedTree`s, or cache parsed trees, in the index. Expect about 10× on Phase I at the cost of index size.
13. Indexing parses every method twice: once in `build_corpus` (extract) and again in `InvertedIndex::build`. Reuse the extracted trees.
14. `Dedup::AllEarlier` is O(T²·len). It is fine with `ClusterTies::First` (T ≤ η2²/2), but slow if `All` is re-enabled.
15. Peak RSS is 1.2 GB during the contiguous recall bench (rayon × parsed trees) and 572 MB during indexing. The index holds the full method text (83.5 MB file).

### Not verified this stage

- Recall at the paper's scale (2.4M methods) and on an Android-app corpus, where the Listing 1/4 style queries would have matches.
- Non-Java recall (paper Table 5). The `ablation` example supports `--lang` (`python`, `javascript`, …) on `data/corpus`, but the Python/JS/Go/Rust corpora are small (2 repos each).

## Reproduce

```sh
cd /home/george/Projects/sillage
export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR=$PWD/target TMPDIR=$PWD/target/tmp; mkdir -p $TMPDIR
cargo build --release --bins --examples
target/release/sillage fetch --manifest corpus.toml --out data/corpus
target/release/sillage index --corpus data/corpus --out data/java.sil --lang java
target/release/sillage stats --index data/java.sil
target/release/sillage bench recall --index data/java.sil --n 1000 --mode contiguous --bootstrap 1000 --seed 7
target/release/sillage bench recall --index data/java.sil --n 1000 --mode noncontiguous --bootstrap 1000 --seed 7
target/release/sillage bench quality --index data/java.sil --n 200 --seed 7
target/release/examples/ablation data/corpus java 200 contiguous 7 misses   # featurizer ablation + misses on stderr
```
