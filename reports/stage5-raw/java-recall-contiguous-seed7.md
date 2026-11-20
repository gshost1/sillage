### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 75.8% [73.2%, 78.5%] | 75.9% [73.1%, 78.6%] | 99.6% [99.1%, 99.9%] |
| Features Search | 81.7% [79.3%, 84.1%] | 82.2% [79.7%, 84.6%] | 99.9% [99.6%, 100.0%] |
| Sillage | 82.4% [79.9%, 84.7%] | 92.9% [91.1%, 94.5%] | 100.0% [100.0%, 100.0%] |

Strict R@1: the original method is ranked first (ties broken by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
