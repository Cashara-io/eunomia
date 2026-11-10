# Connections canvas verification

The Connections route keeps Kinds separate from Objects. Objects reads a bounded, same-space `entities_neighbourhood` response. One filtered scene supplies the canvas, sentence list and complete table. The selected object's values, units, stale and missing states, and document or record sources remain in the existing inspector.

## Canvas polish

The fictional default fixture contains a focus product and 36 neighbours across six kinds. Its 24 orders form a labelled stack. The other 12 neighbours remain individual forms, arranged by kind around the focus. Two hops return 73 total objects; three return 109. The one- and five-neighbour fixtures use a compact canvas.

Height uses the square root of loaded link degree, with a small constant base. Self-links count once. Group height uses the sum of member degrees. Footprint size distinguishes the focus without changing the height scale. No global degree or property value is invented. The legend names the encoding and the summary explains that links outside this loaded neighbourhood may exist.

Each name plate has a Phosphor icon, the kind hue and a stalk ending on its form. Relationship paths intersect the actual isometric top-face polygons. Arrowheads point from the API subject to its destination. Verbs sit on the paths in label plates. Related routes brighten and receive a slow dash on hover, focus or selection. Other routes fade. Reduced motion disables animation and transitions.

The focus has a larger footprint and a ground ring. Neighbours occupy kind sectors, with a quiet kind label for each sector. Contour rings identify each returned hop band. Opening a group reveals already-loaded records and preserves the camera. Fit, zoom and four pan directions are keyboard reachable in a floating icon cluster, with a zoom readout, tooltips and an on-demand shortcut hint. The canvas handles its own question-mark shortcut without also opening the application shortcut dialog.

The header uses one sentence. The larger search offers API-backed suggested objects and session-local recently viewed objects. With no selected object, the inspector column shows neighbourhood counts and kind bars. Hover or keyboard focus on a bar highlights the matching forms. Keyboard focus takes priority over an unrelated pointer hover. At 390 CSS px, this summary or the selected inspector comes before the equivalent table. The table remains scrollable with every loaded object and link.

No dependencies, backend code, migrations or Cargo commands were added.

## Sentence correction

The original small fixture incorrectly used `on_order` for the House blend to Decaf relationship. It now uses `alternative_to`. Sentences always resolve names through the edge's API `from` and `to`, keeping the verb's case and replacing underscore separators with spaces. They do not infer direction from the focus or current selection. Unit and browser tests cover opposite-direction links between the same pair and a self-link. The self-link is also drawn on the canvas.

## Validation

Bun typecheck, lint, all 57 unit tests and the production build passed. The isolated mocked Playwright configuration exercises 24 specs, including the synthetic performance run. Geometry checks cover both themes at 1440, 1280 and 1024 CSS px. They measure the rendered label boxes, confirm that every label stalk starts inside its plate, and check every route endpoint against the rendered top-face perimeter within one CSS pixel. Endpoints must also remain outside their form's label plate.

All ten state fixtures were recaptured at 1440 and 390 in both themes. Populated captures include hover, selection and the table. Additional captures cover all three desktop widths, two and three hops, expanded groups, and tiny scenes. The 72 screenshots, comparison with the Overview and owner reference, measurement JSON and trace are under `/private/tmp/ontology-design/after/pr-4a/`. The reference is used for comparison only; its artwork is not shipped. Three representative fictional-fixture captures are committed under `docs/assets/ontology-connections/`.

These screenshots use desktop Chrome and an emulated phone viewport. They do not establish physical-device performance. At two or three hops, Fit shows the whole neighbourhood at a smaller scale; zoom, keyboard navigation, pan and the full-size table remain available for individual records.

## CI boundary

At the supplied head, the frontend CI job failed in the unchanged proxy upload test: the honest 128 KiB/s MCP upload returned 408 instead of 200. That job uses a backend double. The PR does not modify the proxy, its upload policy or its assertions. The dependent real-backend e2e job was skipped. CI therefore had not reproduced or cleared the earlier local Objects fixture failure, `document.storage_unavailable`, which happened before the page opened on a reused backend. No real-behaviour assertion was weakened, and no backend was built during this polish round. Current-head CI status is reported separately at handoff.

## Performance

The synthetic fixture deliberately exceeds the service contract. It measures a 1,000-object, 3,000-link input using grouping, route aggregation and viewport culling, not all 1,000 forms drawn simultaneously. The service remains bounded at 150 objects and 600 links, with a per-object limit and no neighbourhood paging. Counts and filters describe only returned records.

The final production measurement uses Chrome 156.0.8078.48 on macOS arm64. Interaction timing runs from a trusted click capture to the second animation frame, an upper bound for next paint. It is not a native input-to-paint measurement.

| Measurement | Final result |
| --- | --- |
| Synthetic p95 / worst frame | 16.7 / 16.8 ms |
| Frames over 20 ms | 0 of 601 |
| Incremental JavaScript heap | 8.77 MB |
| Synthetic worker layout | 0.9 ms |
| Supported-size worker layout p95 | 0.4 ms |
| Cold input p95, 30 trials | 26.5 ms |
| Warm input p95, 102 trials | 28.9 ms |
| 4x CPU input p95, 102 trials | 60.8 ms |
| Reload to local content p95, 30 trials | 221 ms |
| Closed-heap growth across three cycles | 1.16% |

The frame distribution sits at the 60 Hz refresh interval. It is not evidence of every frame being strictly below 16.7 ms. Raw results are committed beside the screenshot samples as `performance.json`. Earlier measurement JSON remains in the temporary evidence folder.

