### Recall (contiguous, 1000 queries)

| System | Strict R@1 (95% CI) | §5 R@1 (95% CI) | R@100 (95% CI) |
|---|---|---|---|
| BM25 Keywords | 81.8% [79.4%, 84.1%] | 81.8% [79.3%, 84.0%] | 99.0% [98.3%, 99.6%] |
| Features Search | 67.6% [64.8%, 70.5%] | 67.6% [64.7%, 70.4%] | 99.1% [98.5%, 99.6%] |
| Sillage | 91.1% [89.2%, 93.0%] | 94.7% [93.2%, 96.0%] | 99.4% [98.9%, 99.8%] |

Strict R@1: the original method is ranked first (Sillage orders equal-similarity candidates by full-query overlap, then method size; BM25 and feature search break ties by method id); this is the like-for-like column. §5 R@1 adds the paper's "100% similar" clause: (a) for every system, the rank-1 method has exactly the original's feature multiset (an undeduplicated copy); (b) for Sillage only, the original ties rank 1 at the full query similarity |F(q)|. BM25 and feature search have no score comparable to |F(q)|, so (b) does not apply to them. Partial ties earn no credit.
