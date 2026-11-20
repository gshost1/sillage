### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 81.0% [78.6%, 83.3%] | 81.0% [78.7%, 83.5%] | 99.8% [99.5%, 100.0%] |
| Features Search | 86.1% [83.9%, 88.2%] | 86.4% [84.3%, 88.5%] | 100.0% [100.0%, 100.0%] |
| Sillage | 88.8% [86.9%, 90.6%] | 93.1% [91.6%, 94.6%] | 100.0% [100.0%, 100.0%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
