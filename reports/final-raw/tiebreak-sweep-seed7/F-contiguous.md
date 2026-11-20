### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 75.8% [73.2%, 78.2%] | 75.9% [73.4%, 78.6%] | 99.6% [99.1%, 100.0%] |
| Features Search | 81.7% [79.4%, 83.8%] | 82.2% [80.0%, 84.5%] | 99.9% [99.7%, 100.0%] |
| Sillage | 84.2% [81.8%, 86.2%] | 93.5% [92.0%, 94.8%] | 100.0% [100.0%, 100.0%] |

Strict R@1: the original method is ranked first (ties broken by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
