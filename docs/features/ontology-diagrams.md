# Ontology diagrams in chat

`show_view` accepts a dedicated `diagram` view. A complete graph is a resolved answer, so it belongs beside the existing impact and metric views. The same resolver is also a json-render `diagram` source, drawn by the `Diagram` catalogue component. Both paths return matching structured data and terminal text.

```json
{
  "view": "diagram",
  "diagram": {
    "selection": {
      "type": "path",
      "from_id": "organisation:existing",
      "to_id": "location:existing",
      "hops": "three",
      "direction": "both"
    },
    "emphasis": ["location:existing"],
    "annotations": []
  }
}
```

The example IDs must be replaced with references returned by the authorised vault. No request field accepts nodes, edges, display labels, values, coordinates or colours. Unknown fields are refused at both the direct tool envelope and the nested request boundary. `hops` is `one`, `two` or `three`, defaulting to `two`. Direction is `in`, `out` or `both`, defaulting to `both`.

Selections:

- `neighbourhood`: `focus_id`, optional `hops` and `direction`. Reuses the neighbourhood service and its traversal budgets.
- `path`: `from_id`, `to_id`, optional `hops` and `direction`. Breadth-first selection retains the union of shortest paths in the bounded graph. Every edge retains its stored direction. The graph fetch has a three-second wall-clock deadline in addition to the neighbourhood node, edge and fetched-row caps. A deadline returns only the authorised endpoints as a partial result. A failed search says "No path found within the search limits" and discloses hop, time or traversal truncation.
- `impact`: `focus_id` and optional `hops`. Shares the impact view's traversal and totals. It describes reachability in both directions, without asserting causality.
- `kinds`: `vault_id` and optional `kind`. Uses the ontology's actual kind and directed link counts.

Optional annotations are `{entity_id, metric}` references. The server evaluates approved metrics for that individual object using the existing object metric evaluator, after authorising the selected vault. At most four annotations are allowed. Proposed, unknown and external references fail closed.

The server sorts and places up to twelve nodes and twenty-four edges in `backend/src/views/diagram.rs`, in `Diagram::finish`. Paths use increasing distance along a horizontal chain, with alternative shortest paths in separate lanes. Neighbourhoods place the focus in the centre and sort neighbours by kind around it. Impact uses bands by hop distance. Kinds use the Overview's receding rows and square-root height convention. Omitted nodes are counted by kind. Omitted edges, `partial` and `total_partial` distinguish display limits from traversal limits. Totals under `total_partial` are lower bounds. This is a compact answer, not the interactive Ontology canvas. The table and screen-reader text expose the same saved nodes and links.

The existing chat tool-result storage persists resolved labels, counts, metric values and positions. Reopening chat renders that result without querying the current graph. The renderer shares `LandscapeSolid`, `useLandscapeTint`, `ConnectionKindIcon`, `formPort` and the extracted `boxesIntersect` helper with the Ontology area. `diagram-geometry.ts` derives label plates and edge routes deterministically from the saved coordinates. It also transposes path and impact coordinates on phones. It does not query the graph, randomise positions or run a force simulation. Larger pictures scroll inside a labelled, keyboard-focusable frame, with a visible cue. The initial neighbourhood viewport centres on the focus. There is no animated motion. Layout coordinates are excluded from business-number evidence. Resolved counts and metric values enter the existing evidence registry and checked prose pipeline.

No database queries, migrations, HTTP clients or dependencies were added. The tool result is already an untyped JSON field in OpenAPI, so the OpenAPI schema is unchanged. The frontend API generator was rerun, with no generated API diff. The catalogue generator updates both committed copies.

Bound numeric prose keeps main's exact numeric-span check. A complete custom-unit binding is also accepted at the end of text, a table cell or list item, or before punctuation or a line break. A following letter-word after optional spaces is always refused, including `5 m long`, `42 orders today`, `5 m million`, `5 m squared` and `5 m per second`. Put the complete custom-unit binding at the end of a clause or in a table. To write a natural sentence, bind a bare number and write the unit word yourself, such as a bare `42` binding followed by `orders today`. The earlier word-following exception has been withdrawn. No quantity vocabulary was added. Bare-number behaviour is identical to main. Issue #226 owns the pre-existing gaps for `[5] hundred`, `[12] dozen`, `[3] times`, `[40] percent`, `[5] k`, `[5] million` and `[3] thousand`, where brackets mark the binding.

Historical tool results currently outlive space access, across both chat replay and personal export. That existing behaviour and the policy options are recorded in [issue #230](https://github.com/Cashara-io/eunomia/issues/230). This change does not choose a retention policy.

The catalogue component honours its server-resolved `title` prop for the visible heading and accessible name. A direct diagram view uses the saved diagram title. The catalogue fixture gives it a descriptive title through that same prop, while keeping the shared catalogue coverage assertions unchanged.
