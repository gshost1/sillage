### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 82.6% [80.2%, 85.0%] | 82.6% [80.3%, 84.9%] | 99.8% [99.5%, 100.0%] |
| Features Search | 82.6% [80.2%, 84.9%] | 82.6% [80.2%, 84.9%] | 98.6% [97.9%, 99.3%] |
| Sillage | 83.2% [80.9%, 85.5%] | 86.6% [84.6%, 88.5%] | 98.6% [97.8%, 99.3%] |

Strict R@1: the original method is ranked first (ties broken by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
