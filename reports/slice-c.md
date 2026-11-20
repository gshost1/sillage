# Slice C — `crates/sillage-engine`

Status: implemented. `cargo test -p sillage-engine` passes: 18 tests, 0 warnings.

## Modules
- `prune.rs`: Algorithm 1 as a lazy greedy (CELF). It uses a `BinaryHeap<(gain, Reverse(leaf_idx))>`. The query's features are mapped to dense indices, so computing a gain is a short loop over a slice. A popped leaf is accepted only when its recomputed key is ≥ the top of the heap. Every stored key is an upper bound, because gains never increase as `F` grows, so this picks exactly what the naive argmax picks, including the earliest-index tie-break. Leaves whose gain is 0 are dropped for good. `prune_naive` (`#[cfg(test)]`) plus a 512-case proptest check `keep`, `sim` and `tree` equality.
- `rerank.rs`: Phase II, run in parallel with rayon, sorted by `(sim DESC, overlap DESC, id ASC)`. Score is `sim / |F(q)|`, or 0 for an empty query.
- `cluster.rs`: Phase III clustering, covering Decisions 1, 3, 4, 6 and 7.
  - Takes the τ1 filter, then the first η2 candidates; original `n2` indices are kept.
  - Precomputes `full_f` and `pruned_f`; `pruned_f` comes from re-featurizing the pruned tree.
  - Each tuple carries its running `cs`/`csq` bags.
  - Takes the max of `l` over **all** eligible `j > i_k` that have `csq > 0`. Every candidate tied at the max (within 1e-12) is added if it is also valid.
  - Grows until no new tuple appears.
  - Sorts by `(first ASC, len DESC, tuple ASC)` and applies Jaccard dedup, `AllEarlier` or `KeptOnly`.
  - Public helper `cluster_scored() -> Vec<ScoredCluster { tuple, l, s }>`; `cluster()` maps it to the tuples.
- `intersect.rs`: the recursive `Intersect`. The pair case uses `⊎` (`AsPaper`/`AlwaysSum`) or `∪` (`AlwaysUnion`). The 3+ case re-featurizes the previous tree and uses `∪` (`AsPaper`/`AlwaysUnion`) or `⊎` (`AlwaysSum`).
- `pipeline.rs`: the only module that calls `sillage_parse` and `sillage_index`. It provides `search`, `features_only`, `bm25_search` and `recommend`.
  - `recommend` times each phase.
  - `Recommendation.l`/`s` come from `cluster_scored`, and `query_match` is `prune(fq, rec_tree, featurize(rec_tree)).keep`.
  - Phase I candidates are re-parsed in parallel with `parse_method(rec.lang, &rec.text, rec.body)`, and any that fail to parse are skipped.
  - Records are looked up at `methods[id]`, with a linear-scan fallback when that slot's id doesn't match.
- `lib.rs`: all frozen public types and signatures are unchanged. New exports: `cluster_scored` and `ScoredCluster`.

## Tests (no dependency on parse or index)
`tests/common/mod.rs` provides a tree-building DSL (it collapses single-child nodes per Def. 3, computes labels and spans, and validates). It also provides a local Aroma-style `TestFeaturizer` (token, 3 parent, sibling and var-usage features) and a `TokenFeaturizer`.
- `prune.rs`: lazy `prune` vs the literal Algorithm 1 written with `FeatureBag` ops (proptest); `x=1;y=2;` vs `y=2;z=3;` gives `y = 2 ;`; the swapped nested `if`s keep both `if`s; empty query; multiset capacity.
- `paper_example.rs`: Listing 4 query; Listing 5 and 6; three distractors (one containing the query with extra code, one containing the query with a single call, one unrelated). It checks:
  - the rerank order;
  - that the first cluster is `(0,1)`;
  - that `intersect` gives Listing 7: it keeps `setOnTouchListener … return false` and the `for` loop, and drops `hideKeyBoard()` and `Utils.toggleSoftKeyBoard`;
  - that `PaperLiteral` rejects every cluster.
- `toggles.rs`: growth to the fixed point with exact `l`/`s`; `AllEarlier` vs `KeptOnly` dedup; the `SDenom` toggle; ties add every maximiser; the max of `l` ranges over invalid `j` too (Decision 3); the η2/τ1 limits; all three `IntersectOp` modes on pairs and triples; rerank sort and an empty query.

## Found
- The intersection produced is `… View innerView = …getChildAt(i); (innerView); …`. The paper's Listing 7 also keeps `setupUIToHideKeyBoardOnTouch`. Every feature of a non-variable leaf includes the token's own text, so a callee that appears only in Listing 5 cannot be kept by `Prune(F(L6) ⊎ F(q), L5)`. The paper's listing may reflect presentation rather than the algorithm.
- Greedy prune can keep one stray `view` leaf from the first `if` when pruning Listing 5 against the query. The test allows this; it is consistent with the paper's §5 note. Greedy always reaches the maximum `sim`; only which leaves it picks can vary.
- After pruning, relabelled ancestors can make `SimScore(q, rec)` slightly less than `|F(q)|`: 92/93 in the paper test.
- Environment: `/tmp` (tmpfs) hit its per-user disk quota during the build, even with 1.5G free. I built with `CARGO_TARGET_DIR=target/slice-c` and `TMPDIR=target/slice-c-tmp` inside the repo; both are under the gitignored `/target`. `rustfmt` and `clippy` are not installed on this toolchain, so neither was run.
