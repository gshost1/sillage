# Slice D report: render, bench, CLI, corpus

**Status:** done. `cargo test -p sillage-render -p sillage-bench -p sillage-cli` passes 46/46 with zero warnings. No public signature changed, and `Cargo.lock` / root `Cargo.toml` are untouched.

## Changed
- **`sillage-render`**
  - `layout.rs` copies kept terminals out of `tree.source` in source order, and keeps whitespace exactly when nothing was dropped between two terminals.
  - When whole source lines are dropped, it emits one indented `lang.placeholder()` line (`// ...`, or `...` for Python). An inline drop becomes at most one space.
  - Output is dedented by the common prefix. A line is marked `extra` when it holds unmatched tokens or is a placeholder.
  - With a query it also computes a `similar` line diff (trimmed lines) as `DiffSummary`.
  - Back ends:
    - `ansi.rs`: bold green `+` lines for recommended code, cyan for placeholders.
    - `html.rs`: `<pre class="sillage-rec">` with `line matched|extra|placeholder` spans, escaping `& < > " '`.
    - `json.rs`: `lang`, `code`, `lines[{line_no,text,extra,placeholder,src_line}]` and an optional `diff`.
    - `Plain`: plain text.
  - New exports: `Layout`, `Line`, `DiffSummary`, `layout()`, `diff_summary()`, and `Style: FromStr`.
- **`sillage-bench`**
  - `sample.rs`:
    - Methods qualify with ≥12 non-empty body lines; the outer `{}` is stripped.
    - Sampling uses `ChaCha8Rng(seed)`.
    - Queries: contiguous (first 5 lines), non-contiguous (5 random lines kept in order) and half-split. All are dedented.
  - `bootstrap.rs`: seeded percentile bootstrap that returns the 2.5/97.5 percentiles plus the sample mean.
  - `recall.rs`:
    - Builds the three Table 4 rows from `bm25_search`, `features_only` and `search` at depth 100.
    - `ranked_hits` applies Decision 9: also a hit when the original is in the list and its `sim` equals the top result's.
    - Engine errors count as misses. Runs in parallel with rayon.
  - `quality.rs`: half-snippet queries. Definitions are below.
  - `report.rs`: Markdown tables and JSON for both reports.
  - `RecallMode: FromStr` added.
- **`sillage-cli`**
  - Subcommands: `fetch`, `index`, `query`, `search` (`--show` renders each hit with `pruned.keep`), `explain`, `stats`, `bench recall`, `bench quality`.
  - Tuning flags: `--k --eta1 --eta2 --tau1 --tau2 --tau3`.
  - `--lang` is optional when the file extension identifies the language. With no file, or `-`, the snippet comes from stdin.
  - `manifest.rs` parses `corpus.toml` and rejects bad languages, path-like names, and values starting with `-` (so they can't be read as git options). It shallow-clones each repo, then runs `fetch --depth 1 origin <rev>` and `checkout --detach FETCH_HEAD`.
- **`corpus.toml`**: 14 repos, each pinned to a full commit SHA. I resolved the SHAs from release tags with `git ls-remote` on 2026-11-20.
  - Java: jsoup, junit4, commons-io, commons-lang, retrofit, guava.
  - Python: requests, click.
  - JS / TS: express (JS), zod (TS).
  - Go: cobra, mux.
  - Rust: ripgrep, serde_json.
- **`bench/queries/`**: 13 fixtures.
  - Java: `bitmap_decode.java` (Listing 1), `viewgroup_loop.java` (Listing 4), `package_version.java` (Table 1 Ex. D), `underline_text.java` (Ex. A).
  - Two each for Python, JS, Go and Rust, plus one for TS.

## Interpretation choices (for the lead to confirm)
- **Quality "score":** a `Recommendation` has no score field, so I use containment `|F(q) ∩ F(rec)| / |F(q)|` and compare it with `tau1`.
- **Precision and recall:** computed on `A = F(rec) \ F(q)` (multiset difference) against `H = F(second half)`.
  - The best recommendation per query is picked by F1.
  - Averages cover only queries that got at least one recommendation.
- **Leave-one-out:** quality excludes recommendations whose cluster contains the query's own method, because the original is in the index. It needs ≥8 body lines (`QUALITY_MIN_LINES`) and times queries one after another.
- **`avg_extra_lines`:** the number of distinct source lines holding unmatched tokens, so bench does not need to depend on render.

## Found
- `/tmp` hit its tmpfs user quota (EDQUOT) mid-run. `/tmp/claude-1000` is about 3.7 GB from other sessions, and I deleted nothing. I built with `CARGO_TARGET_DIR=/home/george/Projects/.sillage-target-slice-d` instead of `/tmp/sillage-target-slice-d`, and logs are in `/home/george/Projects/.slice-d-logs/`. Both folders are outside the repo and can be deleted.
- rustfmt and clippy are not installed for the stable toolchain, so neither was run.
- Tests use hand-built trees plus `SimplifiedTree::retain` from core. None of them call parse, index or engine at runtime.
