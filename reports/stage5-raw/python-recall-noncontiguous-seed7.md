### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 88.0% [86.0%, 90.0%] | 88.0% [86.0%, 90.0%] | 99.2% [98.6%, 99.7%] |
| Features Search | 83.5% [81.1%, 85.7%] | 83.5% [81.3%, 85.8%] | 95.8% [94.4%, 97.0%] |
| Sillage | 83.5% [81.3%, 85.9%] | 84.3% [82.0%, 86.5%] | 95.5% [94.1%, 96.7%] |

Strict R@1: the original method is ranked first (ties broken by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
