# Ontology 2: objects and profiles

Source revision: `4325441c207963d405123c69420f4877ade49a5a`. Captured on 2026-11-09 UTC.

This is the branch's production Next.js build and real Axum router on an isolated equivalent Banksia-sized embedded SurrealDB fixture. It is not an export of the live demo. The fixture space has 18,878 entities: 300 CSV products, 12,000 stock records, 6,240 weekly sales records and 338 suppliers. Both large record kinds have typed numeric values. The original 8600/3600 services and database port 8000 were not modified.

The inspected product is Barista coffee basket. Its current CSV source is row 3. Independent Decimal arithmetic gives `(25.90 - 14.90) * 100 = AUD 1,100.00`, with one contributing object. The zero-price product's missing margin remains Missing.

## Screenshots

Every image was opened and inspected after capture. Desktop viewport: 1440 x 900. Phone viewport: 390 x 844. These are full-page captures; the table alone scrolls horizontally on phones. The separate browser journey also checks 320 px for page overflow.

| View | Theme | Viewport width | Actual PNG | Screenshot |
| --- | --- | ---: | --- | --- |
| Directory and desktop inspector | light | 1440 | 1440 x 1586 | [objects](objects-light-1440.png) |
| Profile properties | light | 1440 | 1440 x 1070 | [profile](profile-light-1440.png) |
| Object-scoped numbers | light | 1440 | 1440 x 990 | [numbers](numbers-light-1440.png) |
| Exact current CSV row | light | 1440 | 1440 x 934 | [sources](sources-light-1440.png) |
| Directory and desktop inspector | light | 390 | 390 x 1739 | [objects](objects-light-390.png) |
| Profile properties | light | 390 | 390 x 1219 | [profile](profile-light-390.png) |
| Object-scoped numbers | light | 390 | 390 x 1139 | [numbers](numbers-light-390.png) |
| Exact current CSV row | light | 390 | 390 x 1082 | [sources](sources-light-390.png) |
| Directory and desktop inspector | dark | 1440 | 1440 x 1586 | [objects](objects-dark-1440.png) |
| Profile properties | dark | 1440 | 1440 x 1070 | [profile](profile-dark-1440.png) |
| Object-scoped numbers | dark | 1440 | 1440 x 990 | [numbers](numbers-dark-1440.png) |
| Exact current CSV row | dark | 1440 | 1440 x 934 | [sources](sources-dark-1440.png) |
| Directory and desktop inspector | dark | 390 | 390 x 1739 | [objects](objects-dark-390.png) |
| Profile properties | dark | 390 | 390 x 1219 | [profile](profile-dark-390.png) |
| Object-scoped numbers | dark | 390 | 390 x 1139 | [numbers](numbers-dark-390.png) |
| Exact current CSV row | dark | 390 | 390 x 1082 | [sources](sources-dark-390.png) |

## Endpoint timings

Milliseconds, 12 serial loopback HTTP requests per mode, including response reads and JSON parsing. The first request is reported separately; median and nearest-rank p95 use the following 11 requests. With 11 samples, p95 is the maximum. The process and database were already warm from browser verification. This shared Mac and embedded database are not a production latency guarantee. Lists return 25 rows from the full matching set.

| Read | Matching objects | First | Warm median | Warm p95 |
| --- | ---: | ---: | ---: | ---: |
| kinds | n/a | 305.97 | 303.70 | 305.10 |
| products | 300 | 76.55 | 76.19 | 77.88 |
| stock-name | 12000 | 256.58 | 254.53 | 255.11 |
| stock-page-2 | 12000 | 254.32 | 253.81 | 255.64 |
| stock-deep | 12000 | 255.22 | 254.28 | 256.09 |
| stock-sort | 12000 | 601.68 | 597.98 | 603.01 |
| stock-filter | 100 | 771.42 | 770.22 | 775.35 |
| sales | 6240 | 142.94 | 142.51 | 145.79 |
| sales-sort | 6240 | 322.57 | 322.19 | 324.32 |
| profile | n/a | 29.08 | 27.08 | 28.16 |

Raw [endpoint samples](performance.json) and [browser navigation-to-content timings](render-timings.json) are retained separately. Browser timings include navigation, rendering and shared-machine contention, and are not endpoint timings.

## Checks and limits

- Frontend lint, Next type generation, TypeScript checking and production build passed.
- Both real-backend Playwright tests passed. The browser journey covers paging, filters, column selection, sorting, search reset, refreshed values, scoped metric inputs, sources, related objects and space-preserving navigation.
- 138 backend cases passed across ontology, chat datasets, objects, store and API contract suites. The new endpoint and store suites were rerun after the conflict-query and timestamp refinements. Clippy passed for all targets with warnings denied.
- Membership, vault-token and action-scope bypass mutations were detected. The intermediate service-only scope mutation remained refused by the independent HTTP gate. Guards were restored before final tests and runtime capture.
- Historical imports do not retain their original CSV row number or document revision. The displayed current row is uniquely name-matched and clearly labelled. Timeline is an incomplete record of retained writes and relation creation, with no invented reviews or imports.

PR 3 adds revisioned proposal/decision/value events, durable human reasons, live activity, pending counts, reconnection and revision preconditions. No migration was needed for PR 2.
