# Chat UI visual polish

Frontend head: `8bb67ef4dea26c59c90dfcc2e7030ec390b4f4d3`.

Four catalogue contact sheets cover all 53 registered components in light and dark at 1280 px and 390 px. Each tile was inspected, including every chart. Tall components were captured at an increased viewport height so the whole component is visible. Additional captures cover 10-node and 30-node networks and Card/Grid nesting at both widths.

Validation: frontend lint, TypeScript (`bunx tsc --noEmit`), production build, and all seven tests in `tests/chat-json-render.spec.ts` passed. All 28 chart types were checked by mouse hover and keyboard focus. Line, Bar, Pie, Scatter and Heatmap also assert the formatted `A$120.25` tooltip value. Tests cover empty, single-row and null data, node/label counts, label overlap, donut totals, heatmap scale, sparkline height/axes, nested borders and currency label bounds.

Sunburst is not registered in this branch's catalogue. The uniform Violin density fixture correctly renders with constant width. Backend calculations and catalogue definitions were not changed.
