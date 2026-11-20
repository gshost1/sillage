### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 79.1% [76.6%, 81.5%] | 79.1% [76.4%, 81.6%] | 97.3% [96.2%, 98.3%] |
| Features Search | 76.9% [74.0%, 79.5%] | 76.9% [74.3%, 79.6%] | 96.7% [95.6%, 97.7%] |
| Sillage | 79.0% [76.5%, 81.5%] | 80.2% [77.8%, 82.5%] | 96.6% [95.4%, 97.6%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
