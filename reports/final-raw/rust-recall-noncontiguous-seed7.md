### Recall (non-contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 80.4% [78.1%, 82.8%] | 80.9% [78.5%, 83.3%] | 99.9% [99.7%, 100.0%] |
| Features Search | 79.3% [76.7%, 81.7%] | 80.1% [77.5%, 82.4%] | 98.0% [97.1%, 98.8%] |
| Sillage | 80.3% [77.9%, 82.8%] | 82.7% [80.2%, 84.9%] | 97.4% [96.4%, 98.3%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
