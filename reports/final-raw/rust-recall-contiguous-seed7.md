### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 77.1% [74.5%, 79.5%] | 77.8% [75.2%, 80.6%] | 100.0% [100.0%, 100.0%] |
| Features Search | 84.3% [82.1%, 86.4%] | 85.2% [82.9%, 87.3%] | 99.4% [98.9%, 99.8%] |
| Sillage | 87.6% [85.4%, 89.4%] | 97.0% [95.9%, 98.0%] | 99.4% [98.9%, 99.8%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
