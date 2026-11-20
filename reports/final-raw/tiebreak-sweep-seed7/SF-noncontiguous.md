### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 82.9% [80.8%, 85.1%] | 83.0% [80.9%, 84.8%] | 99.9% [99.7%, 100.0%] |
| Features Search | 79.2% [76.5%, 81.9%] | 79.7% [77.0%, 82.0%] | 99.3% [98.8%, 99.7%] |
| Sillage | 84.2% [82.0%, 86.3%] | 86.6% [84.5%, 88.7%] | 99.2% [98.7%, 99.7%] |

Strict R@1: the original method is ranked first (ties broken by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
