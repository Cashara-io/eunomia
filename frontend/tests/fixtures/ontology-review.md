# Ontology review verification

This fixture uses fictional Banksia Coffee records. All browser API reads and writes are intercepted with `page.route`; the event socket is intercepted too. No backend is needed.

Start an isolated frontend on a free high port, with `BACKEND_INTERNAL_URL` pointing to an unused local endpoint. Set `E2E_BASE_URL` to that frontend and run:

```sh
bunx next typegen
bunx tsc --noEmit
bun run lint
bun test scripts
bunx playwright test --config playwright.ontology-review.config.ts --grep-invert figure
```

To export the paper figures, set `ONTOLOGY_REVIEW_FIGURES` to an output directory and run the same Playwright config without the grep filter. It captures 24 states at 1440 by 900 and 390 by 844, in light and dark. Each PNG has a JSON sidecar with its baseline counterpart, dimensions, browser version, frozen fixture time and SHA-256. Full-page exports can be taller than the viewport.

The focused regression was also run with `Definition.tsx` temporarily restored from the PR base. Selecting a destination produced one mutation when zero was expected. The new implementation passes the same assertion and sends one mutation on final confirmation, including under a double click.

The 16 behaviour tests also check evidence order, queue shortcuts and focus, Escape, owner-only controls, saving, network failure and Retry, stale revision re-review, live refresh during a draft, explicit rejection without a suggested merge, description editing, queue filter/page restoration, 44 pixel queue targets, 200% text with forced colours and reduced-motion reflow at 320, 390 and 1440 pixels.

## Limits

The existing API does not supply the earlier approved definition alongside a proposal, exact source row references for these samples, or record-level merge conflicts. The UI names those gaps. A merge redirects future writes or metric calls. It does not move historical objects or values. Definition-field conflicts are compared locally; the server still checks authority, revision preconditions and dependencies.

Proposer identity and decision actor/time come from filtered activity records. Missing records stay explicitly unavailable. The ontology response alone never supplies an invented actor or timestamp. The source revision is submitted as displayed. The API has no separate destination revision precondition.

The screenshot fixture is smaller than the design audit's full-business fixture. Baseline pairs compare the same task/state, not identical dataset cardinality. They support visual inspection, not measured usability or performance improvements. Shared page chrome belongs to PR A and is intentionally unchanged here.

The updated live integration selectors in `ontology-area.spec.ts` were typechecked, but that suite was not run against a real backend. The mocked suite verifies frontend behaviour only. This work makes no WCAG certification, screen-reader trial, participant study, backend timing or award claim.
