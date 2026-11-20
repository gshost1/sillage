### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 73.1% [70.3%, 75.8%] | 73.3% [70.3%, 76.3%] | 99.5% [99.0%, 99.9%] |
| Features Search | 80.5% [78.0%, 82.9%] | 80.6% [78.2%, 83.0%] | 99.7% [99.3%, 100.0%] |
| Sillage | 84.7% [82.4%, 86.8%] | 93.4% [91.8%, 94.8%] | 99.7% [99.3%, 100.0%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
