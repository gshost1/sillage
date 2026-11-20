### Recommendation quality (half-snippet queries, 200 queries)

| Metric | Value |
|---|---|
| Containment rate (query-mode F(q), ≥ τ1) | 74.0% |
| Avg precision of added features | 46.4% |
| Avg recall of held-out features | 67.0% |
| Avg recommendations / query | 2.79 |
| Avg extra lines / recommendation | 25.47 |
| Exact-match rate (added lines ⊆ held-out) | 0.5% |
| Avg line precision / recall (best rec) | 35.7% / 46.5% |
| Queries with a multi-method cluster | 23.5% |
| Cluster sizes (1 / 2 / 3–5 / 6+) | 495 / 48 / 14 / 2 |
| Latency p50 | 3.6 ms |
| Latency p95 | 28.9 ms |
| Latency max | 47.4 ms |

| Phase | p50 | p95 | max |
|---|---|---|---|
| parse | 0.3 ms | 1.7 ms | 6.5 ms |
| phase1 | 1.4 ms | 1.9 ms | 2.3 ms |
| phase2 | 1.1 ms | 21.6 ms | 35.6 ms |
| phase3 | 0.4 ms | 6.0 ms | 10.3 ms |

Leave-one-out: each query's own method is excluded before Phase II. Containment uses the engine's query-mode F(q); added-feature precision / recall use full featurization of the query, recommendation and held-out half.
