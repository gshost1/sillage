### Recommendation quality (half-snippet queries, 200 queries)

| Metric | Value |
|---|---|
| Containment rate (query-mode F(q), ≥ τ1) | 74.0% |
| Avg precision of added features | 46.6% |
| Avg recall of held-out features | 67.3% |
| Avg recommendations / query | 2.79 |
| Avg extra lines / recommendation | 25.24 |
| Exact-match rate (added lines ⊆ held-out) | 0.5% |
| Avg line precision / recall (best rec) | 35.8% / 46.6% |
| Queries with a multi-method cluster | 24.5% |
| Cluster sizes (1 / 2 / 3–5 / 6+) | 491 / 53 / 12 / 3 |
| Latency p50 | 3.1 ms |
| Latency p95 | 27.3 ms |
| Latency max | 40.9 ms |

| Phase | p50 | p95 | max |
|---|---|---|---|
| parse | 0.2 ms | 1.3 ms | 8.1 ms |
| phase1 | 1.2 ms | 1.5 ms | 2.7 ms |
| phase2 | 1.0 ms | 21.0 ms | 31.3 ms |
| phase3 | 0.3 ms | 5.2 ms | 8.2 ms |

Leave-one-out: each query's own method is excluded before Phase II. Containment uses the engine's query-mode F(q); added-feature precision / recall use full featurization of the query, recommendation and held-out half.
