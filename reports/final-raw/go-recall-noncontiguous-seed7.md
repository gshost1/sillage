### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 88.7% [86.9%, 90.6%] | 88.7% [86.7%, 90.5%] | 99.8% [99.5%, 100.0%] |
| Features Search | 84.0% [81.7%, 86.3%] | 84.3% [82.1%, 86.5%] | 98.4% [97.6%, 99.1%] |
| Sillage | 85.8% [83.7%, 87.8%] | 86.9% [84.7%, 89.1%] | 98.4% [97.6%, 99.2%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
