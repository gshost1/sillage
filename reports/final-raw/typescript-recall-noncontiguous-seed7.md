### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 78.4% [75.8%, 81.0%] | 78.6% [75.9%, 81.2%] | 99.5% [99.0%, 99.9%] |
| Features Search | 70.3% [67.5%, 73.1%] | 70.5% [67.6%, 73.2%] | 98.8% [98.1%, 99.4%] |
| Sillage | 74.5% [71.8%, 77.2%] | 75.5% [73.0%, 78.0%] | 98.6% [97.8%, 99.3%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
