### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 69.6% [66.7%, 72.3%] | 69.7% [67.0%, 72.5%] | 100.0% [100.0%, 100.0%] |
| Features Search | 73.3% [70.7%, 76.0%] | 73.6% [70.7%, 76.4%] | 100.0% [100.0%, 100.0%] |
| Sillage | 77.2% [74.5%, 79.7%] | 91.5% [89.9%, 93.2%] | 100.0% [100.0%, 100.0%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
