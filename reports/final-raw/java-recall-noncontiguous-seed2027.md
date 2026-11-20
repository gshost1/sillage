### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 82.6% [80.2%, 85.0%] | 82.6% [80.3%, 84.9%] | 99.8% [99.5%, 100.0%] |
| Features Search | 82.6% [80.2%, 84.9%] | 82.6% [80.2%, 84.9%] | 98.6% [97.9%, 99.3%] |
| Sillage | 84.3% [82.0%, 86.5%] | 87.3% [85.1%, 89.0%] | 98.8% [98.1%, 99.4%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
