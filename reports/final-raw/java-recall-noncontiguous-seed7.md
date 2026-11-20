### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 82.9% [80.6%, 85.1%] | 83.0% [80.4%, 85.3%] | 99.9% [99.7%, 100.0%] |
| Features Search | 79.2% [76.8%, 81.7%] | 79.7% [77.2%, 82.0%] | 99.3% [98.7%, 99.8%] |
| Sillage | 84.2% [82.0%, 86.6%] | 86.9% [84.9%, 88.9%] | 99.2% [98.6%, 99.7%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
