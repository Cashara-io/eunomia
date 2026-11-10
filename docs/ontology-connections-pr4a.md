# Connections canvas verification

PR4a adds `/ontology/connections` on the existing Ontology shell. Kinds uses the Overview's type landscape. Objects starts with a same-space search and reads `entities_neighbourhood`. It does not write business data.

## Technique

The canvas extends the Overview's SVG approach. `LandscapeSolid` and `useLandscapeTint` are shared by both views. The label plates, focus rings, selection frame and stale-value pin reuse the existing CSS tokens. A small deterministic worker assigns kind and hop lanes. Groups reveal already-loaded objects, with no extra request. Expansion, selection and the table toggle preserve the camera. Camera controls work without dragging. Offscreen marks are culled and the count beyond the frame is explicit. The table retains the complete filtered result.

No dependencies, backend files, migrations or Cargo builds were added.

## Acceptance evidence

| Check | Verification |
| --- | --- |
| Separate Kinds and Objects, directed verbs | Mocked keyboard journey switches modes and asserts that the other node category disappears. Incoming and outgoing fixtures assert exact sentences and table rows. Unit tests cover traversal and the focus position. |
| Count before expansion, bounded requests, deduplication and stability | A 24-member fixture includes duplicate nodes and edges. The test checks the count before opening, unique table rows, unchanged camera and selected URL, and no added request. |
| Same-space filters, counts and table | Search and neighbourhood requests explicitly carry the selected space. One filtered scene supplies both views. Tests compare incoming, outgoing and relation counts. The real-backend spec denies the same object through another space. |
| Keyboard and phone | A keyboard-only search, Tab, Enter and Escape completes inspection. Arrow movement and every labelled pan, zoom and Fit control are exercised. At 390 CSS px the complete table replaces the canvas. The mobile inspector receives focus and closing restores focus to the object. |
| States in both themes | Start, populated, loading, empty, filtered-empty, denied, error, truncated, disconnected and high-degree fixtures are captured at 1440 and 390. Hover, selected, expanded and table captures cover the relevant desktop interactions. Mobile uses the table rather than a hover or cluster interaction. |
| Values and sources | The selected profile endpoint supplies units, missing and stale words, source labels, document rows and record references. The fixture includes AUD values and products.csv row 42. Missing relationship provenance and historical receipts are outside PR4a. |
| Motion | The reduced-motion spec verifies that transitions and animation names are disabled throughout the Connections region. Objects has no continuous animation. |
| Performance | The separate synthetic spec records a 10-second pan, frame distribution, heap, layout durations, 30 cold trials, 102 warm interactions and 102 interactions with 4x CPU slowdown. A Chrome timeline trace records worker/layout and rendering activity. |

## Local validation

Run from `frontend` with Bun. TypeScript, ESLint, all 56 unit tests and the production build passed. The isolated Connections config runs the mocked UI and synthetic performance specs. The real-backend Connections spec also passed against a reused native backend binary with its own in-memory database. No backend was compiled.

The broader Ontology run passed 138 tests and skipped 97 optional figure captures. Its existing Objects spec failed at the `document_upload` fixture, before reaching the page, with `document.storage_unavailable`. A separate rerun reproduced the same storage failure. An earlier performance check also failed heap stability; explicit query-cache disposal and waiting for the closed view fixed it, and the final performance run passed. That spec is unchanged. The reused native backend is not a newly compiled main backend, so CI remains the exact-source integration check.

Three representative fictional-fixture captures are committed under `docs/assets/ontology-connections/`. The final screenshot index, raw performance JSON, retained earlier measurement, Chrome trace and PNGs are under `/private/tmp/ontology-design/after/pr-4a/`. These use fictional API fixtures and an emulated phone viewport. They do not establish real-device or backend throughput.

## API boundary

PR #210 was still open when this branch was built. Main returns `truncated`, with a maximum of 150 objects, 600 links and a per-object edge bound. It has no neighbourhood paging or relation-filter argument. The UI states this limitation. Direction and relation filters operate on the loaded same-space result. Expanding a group cannot claim to fetch unseen neighbours. Reducing depth makes another bounded service read.

The synthetic scene deliberately exceeds this service contract. It measures a 1,000-object, 3,000-link input using grouping, route aggregation and viewport culling. It does not draw all marks simultaneously or claim backend support for that size. Frame times use requestAnimationFrame; interaction times run from a trusted click capture to the second animation frame and are an upper-bound proxy for next paint.



## Reference measurement

Apple M4 Pro, macOS arm64, Chrome 156.0.8078.48, production frontend. The final run recorded:

| Measurement | Result |
| --- | --- |
| Synthetic p95 / worst frame | 16.7 / 16.8 ms |
| Frames over 20 ms | 0 |
| Incremental main-frame JavaScript heap | 8.27 MB |
| Synthetic worker layout | 0.9 ms |
| Supported-size worker layout p95 | 0.3 ms |
| Cold input p95, 30 trials | 27.2 ms |
| Warm input p95, 102 trials | 29.4 ms |
| 4x CPU input p95, 102 trials | 43.9 ms |
| Reload to local content p95, 30 trials | 189 ms |
| Heap growth across three open/close cycles | 0.93% |

The synthetic frame result sits at the 60 Hz refresh interval, rounded to 16.7 ms. It is not evidence of a strictly sub-16.7 ms frame on every refresh. There is no physical midrange-phone measurement.
