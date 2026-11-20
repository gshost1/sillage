# sillage

A clean-room Rust remake of **Aroma: Code Recommendation via Structural Code Search** ([arXiv:1812.01158](https://arxiv.org/abs/1812.01158), OOPSLA 2019).

*Sillage* is the trail of scent a perfume leaves behind — this tool follows the structural code trail other programmers left in a corpus to search and synthesize concise, idiomatic code recommendations via simplified parse trees, sparse structural feature indexing, greedy tree pruning, and constrained cluster-and-intersect.

## Supported Languages

- Java
- Python
- JavaScript
- TypeScript / TSX
- Go
- Rust

## Quick Start

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
cargo test --workspace

# Fetch & index a corpus
sillage fetch --manifest corpus.toml --out data/corpus
sillage index --corpus data/corpus --out data/java.sil --lang java

# Get code recommendations for a snippet
sillage query --index data/java.sil --lang java query.java --format ansi

# Inspect simplified parse tree & structural features
sillage explain --lang java query.java

# Run paper benchmarks (Recall@1 / Recall@100 and Recommendation Quality)
sillage bench recall  --index data/java.sil --n 1000 --mode contiguous --bootstrap 1000 --seed 7
sillage bench quality --index data/java.sil --n 200 --seed 7
```

See [docs/DESIGN.md](docs/DESIGN.md) for architecture details and paper clarifications.
