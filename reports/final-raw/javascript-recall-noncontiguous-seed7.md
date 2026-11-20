### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 80.6% [78.2%, 83.3%] | 80.6% [78.0%, 82.9%] | 99.2% [98.6%, 99.7%] |
| Features Search | 62.7% [59.7%, 65.8%] | 62.7% [59.7%, 65.6%] | 96.3% [95.1%, 97.4%] |
| Sillage | 79.4% [76.8%, 81.8%] | 80.6% [78.2%, 83.0%] | 97.1% [96.0%, 98.1%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
