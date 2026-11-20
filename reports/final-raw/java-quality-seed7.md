### Recommendation quality (half-snippet queries, 200 queries)

| Metric | Value |
|---|---|
| Containment rate (query-mode F(q), ≥ τ1) | 73.0% |
| Avg precision of added features | 46.9% |
| Avg recall of held-out features | 64.2% |
| Avg recommendations / query | 2.65 |
| Avg extra lines / recommendation | 25.49 |
| Exact-match rate (added lines ⊆ held-out) | 3.0% |
| Avg line precision / recall (best rec) | 37.4% / 46.8% |
| Queries with a multi-method cluster | 16.5% |
| Cluster sizes (1 / 2 / 3–5 / 6+) | 490 / 29 / 7 / 4 |
| Latency p50 | 2.9 ms |
| Latency p95 | 24.6 ms |
| Latency max | 34.1 ms |

| Phase | p50 | p95 | max |
|---|---|---|---|
| parse | 0.2 ms | 1.4 ms | 2.8 ms |
| phase1 | 1.1 ms | 1.5 ms | 2.0 ms |
| phase2 | 1.0 ms | 19.1 ms | 27.9 ms |
| phase3 | 0.3 ms | 4.0 ms | 7.1 ms |

Leave-one-out: each query's own method is excluded before Phase II. Containment uses the engine's query-mode F(q); added-feature precision / recall use full featurization of the query, recommendation and held-out half.
