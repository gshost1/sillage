### Recommendation quality (half-snippet queries, 200 queries)

| Metric | Value |
|---|---|
| Containment rate (query-mode F(q), ≥ τ1) | 73.0% |
| Avg precision of added features | 46.5% |
| Avg recall of held-out features | 64.6% |
| Avg recommendations / query | 2.65 |
| Avg extra lines / recommendation | 26.50 |
| Exact-match rate (added lines ⊆ held-out) | 3.0% |
| Avg line precision / recall (best rec) | 37.9% / 48.0% |
| Queries with a multi-method cluster | 16.5% |
| Cluster sizes (1 / 2 / 3–5 / 6+) | 489 / 30 / 9 / 2 |
| Latency p50 | 3.4 ms |
| Latency p95 | 23.3 ms |
| Latency max | 43.6 ms |

| Phase | p50 | p95 | max |
|---|---|---|---|
| parse | 0.2 ms | 1.6 ms | 4.0 ms |
| phase1 | 1.4 ms | 2.0 ms | 2.4 ms |
| phase2 | 1.1 ms | 17.8 ms | 37.8 ms |
| phase3 | 0.4 ms | 4.7 ms | 6.8 ms |

Leave-one-out: each query's own method is excluded before Phase II. Containment uses the engine's query-mode F(q); added-feature precision / recall use full featurization of the query, recommendation and held-out half.
