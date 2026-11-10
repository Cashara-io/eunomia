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
- `path`: `from_id`, `to_id`, optional `hops` and `direction`. Breadth-first selection retains the union of shortest paths in the bounded graph. Every edge retains its stored direction. A failed search says no path was found within the hop limit, and discloses traversal truncation.
- `impact`: `focus_id` and optional `hops`. Shares the impact view's traversal and totals. It describes reachability in both directions, without asserting causality.
- `kinds`: `vault_id` and optional `kind`. Uses the ontology's actual kind and directed link counts.

Optional annotations are `{entity_id, metric}` references. The server evaluates approved metrics for that individual object using the existing object metric evaluator, after authorising the selected vault. At most four annotations are allowed. Proposed, unknown and external references fail closed.

The server sorts and places up to twelve nodes and twenty-four edges on a fixed two-column plane. Omitted nodes are counted by kind. Omitted edges, `partial` and `total_partial` distinguish display limits from traversal limits. Totals under `total_partial` are lower bounds. This is a compact answer, not the interactive Ontology canvas. The table and screen-reader text expose the same saved nodes and links.

The existing chat tool-result storage persists resolved labels, counts, metric values and positions. Reopening chat renders that result without querying the current graph. Theme tokens select the Overview's kind colours and icons. Layout coordinates are excluded from business-number evidence. Resolved counts and metric values enter the existing evidence registry and checked prose pipeline.

No database queries, migrations, HTTP clients or dependencies were added. The tool result is already an untyped JSON field in OpenAPI, so the OpenAPI schema is unchanged. The frontend API generator was rerun, with no generated API diff. The catalogue generator updates both committed copies.
