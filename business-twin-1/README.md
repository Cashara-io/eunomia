# Business twin PR 1 proof

Implementation head: `68285daf763f4a63690b6f94169f4fdd427d8cbf`. Base: `6116a00ff86fed992a917863e9a3515f70373b84`.

Fictional Banksia Basket Group, real Eunomia 2.2.0 API on port 8600. The branch frontend ran on port 3610. All 16 full-page screenshots were captured through local Playwright with Helium, device scale 1, at 1440x900 and 390x844 CSS viewports. Full page heights are recorded in captures.json. Each PNG was opened and visually checked. Zero Banksia business writes or decisions were made.

Five twin tests and nine related regressions passed against the real backend, using separate registered fixture accounts and MCP-seeded spaces. No-model and outage response branches use browser response fixtures. Frontend lint, typecheck and production build passed. Keyboard navigation and 320 px overflow are covered by the twin tests. The shared browser snapshot tool timed out twice before local capture took over.

No backend changes or migrations. Batch reasons are local downloadable receipts, explicitly not stored by the 2.2.0 API. Dated proposal/decision history and atomic revision checking are unavailable.

| View | Desktop light | Desktop dark | Phone light | Phone dark |
|---|---|---|---|---|
| Overview | [1440 light](overview-1440-light.png) | [1440 dark](overview-1440-dark.png) | [390 light](overview-390-light.png) | [390 dark](overview-390-dark.png) |
| Structure List | [1440 light](structure-1440-light.png) | [1440 dark](structure-1440-dark.png) | [390 light](structure-390-light.png) | [390 dark](structure-390-dark.png) |
| Product kind | [1440 light](kind-1440-light.png) | [1440 dark](kind-1440-dark.png) | [390 light](kind-390-light.png) | [390 dark](kind-390-dark.png) |
| Review detail | [1440 light](review-1440-light.png) | [1440 dark](review-1440-dark.png) | [390 light](review-390-light.png) | [390 dark](review-390-dark.png) |
