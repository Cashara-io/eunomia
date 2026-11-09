# Business twin: first release

Business twin lives under `/twin`, scoped by the `space` URL parameter. Only Overview, Structure and Review are exposed. Kind details have their own routes. Search, filters, paging, kind section and review selection are URL state, so browser Back restores them.

The former Kinds and links controls now live in Business twin. Memory spaces retains a compact read-only summary. The old ontology, kinds-and-links, kinds and links anchors redirect to Structure for the open space. CSV guidance now names Business twin. Add data reuses Auto-map and links to Documents with the same space selected. Import remains an explicit action in the existing CSV preview.

Structure and Review page complete API results in groups of ten. Kind properties, numbers and incoming/outgoing links have separate sections with bounded, expandable details. No canvas or object directory is included in this release.

## Decisions and API limits

Owner checks remain on the existing server endpoints. The moved rows retain status words, description edits, built-in restrictions, explicit merge targets and metric previews. Batch decisions run sequentially with kinds before properties, links and metrics. Each item is checked against a fresh read before submission. Failed or changed items have individual results. Batch rejection never merges.

The 2.2.0 API cannot persist human review reasons, dated decisions or atomic revision preconditions. A required batch reason appears in a downloadable local receipt, explicitly labelled as unsaved on the server. It disappears on leaving the page unless downloaded. This is not a durable audit log. A simultaneous owner decision can still race the final check. Durable reasons, events and revision checks belong to PR 3 of the research plan.

Readiness uses the existing backend rule. Proposal dates, actor identities beyond origin, source mappings and examples are shown only when returned. Missing history and provenance are stated plainly. The existing change stream invalidates ontology queries on relevant events; Refresh remains available because the old backend does not publish every proposal-only change.

## Validation

- Frontend ESLint, TypeScript and production build.
- Five real-backend Playwright tests in `frontend/tests/business-twin.spec.ts`, seeded through MCP into isolated test spaces using the existing registration/onboarding helper. They cover routes, paging, filters, Back, kind sections, Add data destination, owner/member controls, approval/rejection/merge, receipts, a four-item batch with dependencies, built-in restrictions, keyboard navigation, 390/320 px overflow, unavailable data and no-model copy. Outage and no-model response branches use browser response fixtures; the other assertions use real server responses.
- Nine regression tests in vaults, documents and live-updates.
- Banksia Basket Group screenshots at 1440 and 390 CSS px, both themes, for Overview, Structure List, product kind detail and Review. Full-page PNGs use device scale 1. Each was visually inspected. No decisions or other business writes were made in Banksia.
- Shared preview snapshots timed out twice. Captures then used local Playwright with Helium. No walk-me-through skill was installed. Design verification was performed in-thread because the implementation sub-agent was instructed not to delegate.

PR 2 adds typed object tables, full object profiles, source/as-of facts and contributing metric inputs. The later graph, full Numbers/Coverage views and impact tools remain outside this release.
