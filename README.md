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
| `sillage serve [--host H] [--port 8765] [--corpus NAME=PATH]... [--root DIR] [--token T]` | MCP server over Streamable HTTP for web-hosted agents (see [MCP server](#mcp-server-for-coding-agents)). |
| `sillage mcp [--corpus NAME=PATH]... [--root DIR]` | MCP server over stdio, launched by a local agent. |

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

## MCP server for coding agents

sillage serves its agent tools over the [Model Context Protocol](https://modelcontextprotocol.io) (protocol 2025-06-18 and 2025-03-26). Corpora stay indexed in RAM. A directory corpus is re-indexed incrementally, so only changed files are re-parsed.

```sh
# Web-hosted (Streamable HTTP): http://HOST:8765/mcp, health at /healthz
export SILLAGE_MCP_TOKEN=$(openssl rand -hex 32)
sillage serve --host 0.0.0.0 --port 8765 --root ~/src/app --refresh-interval 120

# Several corpora: live directories and/or prebuilt .sil snapshots
sillage serve --corpus api=~/src/api --corpus web=~/src/web --corpus java=data/java.sil --default-corpus api

# Local (stdio), launched by the agent inside the repository
sillage mcp --root .
```

| Flag (`serve` / `mcp`) | Meaning |
|---|---|
| `--corpus NAME=PATH` | A corpus: a directory (indexed live) or a `.sil` file (static). Repeatable. |
| `--root DIR` | A directory served under its basename and used as the default corpus. With no `--root` or `--corpus`, the current directory is served. |
| `--default-corpus NAME` | The corpus used when a call names none. Defaults to `--root`, else the first `--corpus`. |
| `--lang L[,L...]` | Languages indexed in directory corpora. Default: all 7. |
| `--refresh-interval SECS` | Background incremental refresh. `0`, the default, refreshes only on `refresh_corpus`. |
| `--host`, `--port` (`serve`) | Bind address. Default `127.0.0.1:8765`. |
| `--token T` (`serve`) | Requires `Authorization: Bearer T` on `/mcp`. Defaults to `$SILLAGE_MCP_TOKEN`; prefer the variable, because arguments show up in `ps`. The token is compared in constant time and never logged. |
| `--cors-origin O` (`serve`) | A browser origin allowed to call `/mcp` (repeatable; `*` allows any). Browser requests from any other origin get `403`. Clients that send no `Origin` header (CLI agents) are always allowed. |
| `--max-body-bytes N` (`serve`) | Request size limit. Default 2 MiB. Larger requests get `413`. |

**Endpoints:**
- `POST /mcp` takes JSON-RPC: `200` JSON for requests, `202` for notifications.
- `GET /mcp` opens an SSE keep-alive stream.
- `DELETE /mcp` ends a session.
- `OPTIONS /mcp` answers CORS preflights.
- `GET /healthz` returns `{"status":"ok"}` without auth.

`initialize` issues an `Mcp-Session-Id`. A request that carries a session id that is unknown or was deleted gets `404`. Requests without a session id are served statelessly, because every tool depends only on its arguments. Sessions expire after 1 h idle, and at most 1024 are kept.

### Tools

| Tool | When an agent calls it |
|---|---|
| `find_idioms` | After drafting a body or snippet. Returns the repo's consensus completion as a diff in the agent's own variable names (`detail`: `diff`, `compact` or `full`), with confidence, cluster size, `file:line` provenance and the imports the added lines need (`preset`: `repo` or `paper`). |
| `find_similar_code` | Before writing a helper that may already exist. Finds structurally similar methods (`scope`: `all`, `prod` or `test`), each with `file:line`, a prod/test badge and a score. |
| `check_missing_guards` | On finished `code` + `lang`, or on a raw `git diff`. Reports only high-confidence error / null / bounds checks, cleanup, logging and validation that a consensus of similar production methods has and the code lacks. Stays silent when it finds none. |
| `find_test_idioms` | When writing tests. Returns repo tests to model on, with skeletons renamed to the new function, how each test was linked, assertions, fixtures and test imports. |
| `corpus_status` | Shows what is indexed: method counts (prod / test), files, languages and last refresh. Host paths are never shown. |
| `refresh_corpus` | After editing files. Runs an incremental re-index and returns files added / modified / removed / unchanged, total methods and elapsed time. |

Every tool except the two corpus tools takes `corpus` (optional) and `exclude_path` (pass the file being edited, so it never matches itself). Inputs are capped: 64 KiB per snippet, 1 MiB per diff, `limit` ≤ 20, `max_lines` ≤ 400.

### Client configuration

Replace `https://sillage.example.com/mcp` with your server's URL and export `SILLAGE_MCP_TOKEN` in the client's environment.

**Claude Code.** Remote:

```sh
claude mcp add --transport http sillage https://sillage.example.com/mcp \
  --header "Authorization: Bearer $SILLAGE_MCP_TOKEN"
```

Local, run from the repository:

```sh
claude mcp add sillage -- sillage mcp --root .
```

Or check a project-scoped `.mcp.json` into the repository:

```json
{
  "mcpServers": {
    "sillage": {
      "type": "http",
      "url": "https://sillage.example.com/mcp",
      "headers": { "Authorization": "Bearer ${SILLAGE_MCP_TOKEN}" }
    },
    "sillage-local": { "command": "sillage", "args": ["mcp", "--root", "."] }
  }
}
```

**Codex** (`~/.codex/config.toml`):

```toml
[mcp_servers.sillage]
url = "https://sillage.example.com/mcp"
bearer_token_env_var = "SILLAGE_MCP_TOKEN"

[mcp_servers.sillage-local]
command = "sillage"
args = ["mcp", "--root", "."]
```

**Cursor** (`.cursor/mcp.json`, or `~/.cursor/mcp.json` for all projects):

```json
{
  "mcpServers": {
    "sillage": {
      "url": "https://sillage.example.com/mcp",
      "headers": { "Authorization": "Bearer ${env:SILLAGE_MCP_TOKEN}" }
    },
    "sillage-local": { "command": "sillage", "args": ["mcp", "--root", "${workspaceFolder}"] }
  }
}
```

**T3 Code** runs Claude Code and Codex sessions, and each session loads its provider's MCP configuration. Register sillage for the providers you use: the Claude Code `.mcp.json` / `claude mcp add` entries above (remote or local) and the Codex `config.toml` tables above. New T3 Code threads in that project then get the six tools.

### Deployment

**Docker.** With no corpus flags, the image serves the mounted `/workspace`:

```sh
docker build -t sillage .
docker run -d --name sillage -p 8765:8765 -e SILLAGE_MCP_TOKEN \
  -v "$HOME/src/app:/workspace:ro" sillage \
  serve --host 0.0.0.0 --port 8765 --refresh-interval 120
```

The image runs as a non-root `sillage` user, exposes 8765 and has a `HEALTHCHECK` on `/healthz`.

**systemd.** [`deploy/sillage.service`](deploy/sillage.service) is a hardened unit (`ProtectSystem=strict`, `NoNewPrivileges`, `PrivateTmp`, no capabilities). It reads the token from `/etc/sillage/sillage.env`. Install steps are in the unit's header comment. For anything beyond localhost, put a TLS-terminating reverse proxy in front.

## Results (Stage 6, n = 1000, bootstrap 1000)

Strict R@1 means the original method is ranked first; this is the like-for-like column. Sillage orders equal-similarity candidates structurally: first by overlap with the query's full featurization, then by method size, both computed on alpha-renamed features (DESIGN Decision 24). BM25 and feature search break ties by method id.

§5 R@1 adds the paper's "100% similar" clause:
- for every system, a rank-1 method with exactly the original's features;
- for Sillage only, a tie with rank 1 at the full query similarity |F(q)|.

Seed 7 is the development seed, and the tie-break was chosen on it. Seed 2027 is held out. Details, CIs and the Stage 5 → Stage 6 comparison: [reports/final-benchmarks.md](reports/final-benchmarks.md).

**Java, paper Table 4** (80,192 methods):

| System | Mode | Strict R@1 (s7 / s2027) | §5 R@1 (s7 / s2027) | R@100 (s7 / s2027) | Paper R@1 / R@100 |
|---|---|---|---|---|---|
| BM25 | contiguous | 75.8% / 73.1% | 75.9% / 73.3% | 99.6% / 99.5% | 78.3% / 96.9% |
| Features | contiguous | 81.7% / 80.5% | 82.2% / 80.6% | 99.9% / 99.7% | 78.3% / 96.8% |
| **Sillage** | contiguous | **85.8% / 84.7%** | **94.0% / 93.4%** | **100.0% / 99.7%** | 99.1% / 100% |
| BM25 | non-contiguous | 82.9% / 82.6% | 83.0% / 82.6% | 99.9% / 99.8% | 93.0% / 99.9% |
| Features | non-contiguous | 79.2% / 82.6% | 79.7% / 82.6% | 99.3% / 98.6% | 88.1% / 98.6% |
| **Sillage** | non-contiguous | **84.2% / 84.3%** | **86.9% / 87.3%** | **99.2% / 98.8%** | 98.3% / 100% |

Stage 5 (ties broken by method id) had Sillage strict R@1 82.4% / 81.3% contiguous and 79.8% / 83.2% non-contiguous. The paper's 99.1% / 98.3% are matched only under the pre-Stage 5 "any equal-similarity tie" rule (99.2% / 95.5% at seed 7). That rule credits partial ties, so it is no longer used.

**Multi-language Table 5** (seed 7, Sillage):

| Language | Methods | Strict R@1 contig / non-contig | §5 R@1 contig / non-contig | R@100 contig / non-contig |
|---|---|---|---|---|
| Python | 8,554 | 79.0% / 85.8% | 80.2% / 86.5% | 96.6% / 95.3% |
| JavaScript | 9,347 | 91.1% / 79.4% | 94.7% / 80.6% | 99.4% / 97.1% |
| TypeScript | 5,592 | 77.2% / 74.5% | 91.5% / 75.5% | 100.0% / 98.6% |
| Go | 3,729 | 88.8% / 85.8% | 93.1% / 86.9% | 100.0% / 98.4% |
| Rust | 8,677 | 87.6% / 80.3% | 97.0% / 82.7% | 99.4% / 97.4% |

**Quality and latency (Java, n = 200):**
- Seed 7: 73.0% of queries contained (≥ τ1), 2.65 recommendations per query, 16.5% with a multi-method cluster.
- Seed 2027: 74.0%, 2.79 and 24.5%.
- `recommend` latency: p50 2.8–2.9 ms, p95 23–24 ms (Stage 5 binary in the same session: p50 2.7–2.8 ms, p95 20–21 ms).

## Documentation

- [docs/DESIGN.md](docs/DESIGN.md): architecture, crate graph, how each paper ambiguity was resolved (Decisions 1–24), and index format v6 (xxh3 checksum, per-method sizes, test flags and per-file imports, validation on load).
- [reports/final-benchmarks.md](reports/final-benchmarks.md): Stage 5 benchmarks and the list of review fixes. Earlier stages are in `reports/`.

## License

MIT, see [LICENSE](LICENSE).
