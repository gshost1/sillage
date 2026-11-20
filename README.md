# sillage

A clean-room Rust remake of **Aroma: Code Recommendation via Structural Code Search** (Luan et al., OOPSLA 2019, [arXiv:1812.01158](https://arxiv.org/abs/1812.01158)).

*Sillage* is the trail of scent a perfume leaves behind. Given a partial code snippet, sillage finds methods in a corpus that contain it structurally and returns concise recommendations for how such code usually continues. It uses simplified parse trees, sparse structural features, an inverted index, greedy pruning and cluster-and-intersect.

Languages: Java, Python, JavaScript, TypeScript / TSX, Go, Rust.

## Build and install

```sh
cargo build --release              # binary at ./target/release/sillage
cargo install --path crates/sillage-cli   # or install `sillage` into ~/.cargo/bin
cargo test --workspace --release
```

The examples below call `sillage`; use `./target/release/sillage` if you did not install it.

## Quick start

```sh
# 1. Clone the pinned corpus (corpus.toml) and index one language
sillage fetch --manifest corpus.toml --out data/corpus
sillage index --corpus data/corpus --out data/java.sil --lang java

# 2. Recommend code for a snippet
sillage query --index data/java.sil bench/queries/viewgroup_loop.java
sillage query --index data/java.sil bench/queries/bitmap_decode.java --format plain --max-lines 20

# 3. Structural search (Phase I + II), optionally rendered
sillage search --index data/java.sil bench/queries/bitmap_decode.java --n 5 --show

# 4. Snippets from stdin need --lang
echo 'with open(path) as fh:
    data = json.load(fh)' | sillage query --index data/python.sil --lang python -
```

Query fixtures for every language are in `bench/queries/`, for example `viewgroup_loop.java` (paper Listing 4), `bitmap_decode.java`, `read_json.py`, `fetch_json.js`, `debounce.ts`, `open_file.go` and `read_lines.rs`.

## Commands

| Command | What it does |
|---|---|
| `sillage fetch [--manifest corpus.toml] [--out corpus]` | Shallow-clones every repo in the manifest at its pinned commit. |
| `sillage index --corpus DIR --out FILE.sil [--lang L ...]` | Extracts, deduplicates, featurizes and indexes every method (all languages by default). Fails if `DIR` is missing or yields no methods. Reports unreadable files on stderr. |
| `sillage query --index FILE [SNIPPET\|-]` | Code recommendation (Phases I–III): up to `k` clustered, intersected recommendations. Recommended lines are marked. |
| `sillage search --index FILE [SNIPPET\|-] [--n 10] [--show]` | Structural search: the top `n` methods by pruned similarity. `--show` renders each one with the query-matching lines unmarked. |
| `sillage explain [SNIPPET\|-]` | Prints the snippet's simplified parse tree and per-leaf query features. |
| `sillage stats --index FILE` | Methods per language, repos, features, postings. |
| `sillage bench recall --index FILE [--n 1000] [--mode contiguous\|noncontiguous] [--bootstrap 1000] [--seed 7] [--json]` | Paper §5 / Table 4: strict and §5 Recall@1 and Recall@100 for BM25, feature search and Sillage, with 95% bootstrap CIs. |
| `sillage bench quality --index FILE [--n 200] [--seed 7] [--json]` | Paper §4.2: recommendation quality on half-method queries, leave-one-out, with per-phase latency. |

**Snippet input.** Pass a file, or `-` / nothing for stdin. The language comes from `--lang` (`java`, `python`/`py`, `javascript`/`js`, `typescript`/`ts`, `tsx`, `go`, `rust`/`rs`). Without `--lang` it is taken from the file extension, mapped as `index` maps corpus files (`.pyi`, `.mjs`, `.mts`, … included). Stdin needs `--lang`, and its absence is reported before stdin is read.

**Output** (`query`, `search --show`): `--format plain|ansi|html|json` (default `ansi`).
- `ansi` marks recommended lines with a green `+` gutter and prefixes file line numbers. Control characters from the corpus are escaped.
- `html` emits a `<pre class="sillage-rec">` with `data-line` attributes.
- `json` is one document:
  - `query`: `{recommendations: [{cluster: [{id, repo, path, name, line_start}], l, s, render}], timings}`;
  - `search`: `{lang, results: [{rank, id, repo, path, name, line_start, score, sim, overlap, render?}]}`.
- `--max-lines N` truncates each rendering to N lines plus a `// ... (K more lines)` placeholder.

**Algorithm parameters** (`query`, `search`, `bench`), paper defaults in brackets:

| Flag | Meaning |
|---|---|
| `--k` [5] | recommendations returned (≥ 1) |
| `--eta1` [1000] | Phase I candidates (≥ 1) |
| `--eta2` [100] | Phase III candidates from the reranked list (≥ 1) |
| `--tau1` [0.65] | minimum pruned similarity `sim / \|F(q)\|`, in [0, 1] |
| `--tau2` [1.5] | minimum cluster extension ratio `l(τ)`, finite ≥ 0 |
| `--tau3` [0.9] | minimum cluster query alignment `s(τ)`, in [0, 1] |

## Results (Stage 5, n = 1000, bootstrap 1000)

Strict R@1 means the original method is ranked first; this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause:
- for every system, a rank-1 method with exactly the original's features;
- for Sillage only, a tie with rank 1 at the full query similarity |F(q)|.

Seed 7 is the development seed; seed 2027 is held out. Details and CIs: [reports/final-benchmarks.md](reports/final-benchmarks.md).

**Java, paper Table 4** (80,192 methods):

| System | Mode | Strict R@1 (s7 / s2027) | §5 R@1 (s7 / s2027) | R@100 (s7 / s2027) | Paper R@1 / R@100 |
|---|---|---|---|---|---|
| BM25 | contiguous | 75.8% / 73.1% | 75.9% / 73.3% | 99.6% / 99.5% | 78.3% / 96.9% |
| Features | contiguous | 81.7% / 80.5% | 82.2% / 80.6% | 99.9% / 99.7% | 78.3% / 96.8% |
| **Sillage** | contiguous | **82.4% / 81.3%** | **92.9% / 91.5%** | **100.0% / 99.7%** | 99.1% / 100% |
| BM25 | non-contiguous | 82.9% / 82.6% | 83.0% / 82.6% | 99.9% / 99.8% | 93.0% / 99.9% |
| Features | non-contiguous | 79.2% / 82.6% | 79.7% / 82.6% | 99.3% / 98.6% | 88.1% / 98.6% |
| **Sillage** | non-contiguous | **79.8% / 83.2%** | **84.5% / 86.6%** | **99.3% / 98.6%** | 98.3% / 100% |

The paper's 99.1% / 98.3% are matched only under the pre-Stage 5 "any equal-similarity tie" rule (99.2% / 95.5% at seed 7). That rule credits partial ties, so it is no longer used.

**Multi-language Table 5** (seed 7, Sillage):

| Language | Methods | Strict R@1 contig / non-contig | §5 R@1 contig / non-contig | R@100 contig / non-contig |
|---|---|---|---|---|
| Python | 8,554 | 77.4% / 83.5% | 78.9% / 84.3% | 96.7% / 95.5% |
| JavaScript | 9,347 | 68.1% / 62.5% | 86.7% / 67.7% | 99.5% / 96.6% |
| TypeScript | 5,592 | 73.5% / 70.4% | 89.1% / 71.6% | 100.0% / 98.7% |
| Go | 3,729 | 86.3% / 84.4% | 91.5% / 85.4% | 100.0% / 98.4% |
| Rust | 8,677 | 84.7% / 79.6% | 96.6% / 82.3% | 99.4% / 97.2% |

**Quality and latency (Java, n = 200):**
- Seed 7: 73.0% of queries contained (≥ τ1), 2.65 recommendations per query, 16.5% with a multi-method cluster.
- Seed 2027: 74.0%, 2.79 and 23.5%.
- `recommend` latency: p50 3.4 ms, p95 23.3 ms.

## Documentation

- [docs/DESIGN.md](docs/DESIGN.md): architecture, crate graph, how each paper ambiguity was resolved (Decisions 1–23), and index format v4 (xxh3 checksum, validation on load).
- [reports/final-benchmarks.md](reports/final-benchmarks.md): Stage 5 benchmarks and the list of review fixes. Earlier stages are in `reports/`.

## License

MIT, see [LICENSE](LICENSE).
