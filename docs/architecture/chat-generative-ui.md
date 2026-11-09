# Chat generative UI

`show_view` accepts `view: "ui"` alongside impact, subgraph, metric and scenario. Keeping the existing tool preserves the chat stream, stored history and MCP structured-content path. Every result carries a plain `text` answer.

The model supplies a json-render tree and source queries. The server resolves sources, checks the tree against the catalog and checks every bound path and column. It never accepts inline data or model-supplied numeric props. The browser uses json-render 0.22.0 for rendering and lazy-loads visx 4 for charts. The chat model remains `gpt-6-luna`.

## Catalog

The authoritative definition is `frontend/src/components/chat/ui/catalog.ts`, using json-render's catalog API and Zod. `bun scripts/chat-catalog.ts`, run inside `frontend`, generates the prompt, JSON schema and prop schemas. The generated JSON lives inside both build contexts:

- `frontend/src/components/chat/ui/catalog.generated.json`
- `backend/src/views/catalog.generated.json`

`bun scripts/chat-catalog.ts --check` rejects stale or different copies. The Playwright catalog test runs this check in CI.

There are 53 components, including 28 chart types. The catalog includes layouts, copy, stats, sortable tables with pagination and server totals, entity links, meters, timelines, comparisons, the four legacy blocks, and the complete chart list in `chartNames`. Sankey uses the maintained `@visx/sankey` 4.0.0 package. Cyclic flows show a notice directing the caller to Network.

## Sources

A `sources` object has one to eight named entries. Names contain only ASCII letters, digits and underscores. The resolved state is `{sources: {...}}`.

| Type | Arguments | Resolved data |
| --- | --- | --- |
| `dataset` | `vault_id`, `kind`, `properties`, optional `filters`, `group_by`, `bucket`, `aggregates`, `sort`, `limit` | `rows`, typed `columns`, `totals`, `distributions`, `status`, `total_rows`, `truncated`, `warnings` |
| `metric` | The `metric_eval` arguments, flattened beside `type` | The existing resolved metric view, including its series, plus approval status and warnings |
| `scenario` | `headline` and `scenario`, containing the `scenario_run` arguments | The existing resolved scenario view and its warnings |
| `impact` | The existing impact view arguments beside `type` | The existing resolved impact view |
| `subgraph` | The existing subgraph view arguments beside `type` | The existing resolved subgraph view |

Dataset filters reuse metric filter validation. Properties must be approved. Strict mode also requires an approved custom kind. Reads use the existing org handle and vault authorisation. Unreviewed kinds outside strict mode are labelled `not approved`. An explicitly requested proposed metric keeps its `not approved` status even when drawn as a generic stat.

`group_by` is one property key, or `link:label` for an approved outgoing link. Link columns contain target entity references. Targets remain inside the authorised vault. A date property can use `day`, `week`, `month`, `quarter` or `year` bucketing. Aggregates are `count`, `sum`, `avg`, `min`, `max`, `median` and `distinct`. Numeric aggregates retain the property's unit. Counts have no unit. Nulls are excluded from numeric aggregates. A count without a property counts rows.

Omit `aggregates` for a detail table. Totals are always computed separately, over the selected rows before output pagination. For a table total, bind the whole dataset and set the column's `total` to `sum`, `avg` or `count`.

The server also computes histogram bins, box plot statistics and violin counts. These displayed values never come from browser aggregation. Their bindings are:

- Histogram: `/sources/name/distributions/property/bins`
- BoxPlot: `/sources/name/distributions/property/box`
- Violin: `/sources/name/distributions/property/density`

Other charts normally bind `/sources/name/rows`. Sankey reads its source key from `x`, target key from `group` or `target`, and value key from `y`. Tree, Treemap and Partition can read a `parent` key. Bubble can read a `size` key. Units and date formatting come from the typed source columns, with the existing number-formatting functions used for display.

## Spec example

```json
{
  "view": "ui",
  "sources": {
    "items": {
      "type": "dataset",
      "vault_id": "vault:example",
      "kind": "item",
      "properties": ["date", "amount"],
      "group_by": "date",
      "bucket": "month",
      "aggregates": [{"op": "sum", "property": "amount"}]
    }
  },
  "spec": {
    "root": "chart",
    "elements": {
      "chart": {
        "type": "Line",
        "props": {
          "title": "Amount by month",
          "summary": "Monthly amount from approved properties",
          "data": {"$state": "/sources/items/rows"},
          "x": "date",
          "y": "sum_amount"
        },
        "children": []
      }
    }
  }
}
```

## Limits and failure behaviour

Specs allow 100 elements, depth 12 and 64 KiB. Actions, repeats, arbitrary state, visibility expressions, unreachable elements, shared children and cycles are refused. Source output is capped at 2 MiB. Datasets read at most 5,000 entities and return at most 500 rows. When input is capped, warnings state that aggregates cover partial input. Output truncation is reported separately.

Unknown components, invalid props and missing bindings show an inline notice and the plain source answer. Chart code is loaded only when needed. Both themes use existing design tokens. Charts provide accessible names, summaries, keyboard and pointer tooltips, and legends for multiple series. Recharts remains in `frontend/src/app/(app)/settings/UsageSection.tsx`; chat has no Recharts imports.

## Model prose and resolved chart columns

Model titles, captions, labels, legends, column headers and tab names may contain ordinary words in any language. Number words such as "twice" and Roman words such as "IV" are not checked. Only decimal digits are checked, after Unicode compatibility normalisation and converting decimal digits in every script to ASCII. This also checks digit forms such as superscript and circled digits. Digits in alphanumeric identifiers such as Q3, A4, COVID-19, 4WD and B2B are allowed. Other digit quantities must match a value in this view's resolved server data or an applied parameter such as a limit or date range. Matching removes currency symbols and thousands separators, rounds to the displayed precision and accepts percentages expressed as fractions or whole percentages. Rejections name the offending field.

The server returns explicit x, y and series columns for every chart. An omitted y is resolved only when server metadata identifies exactly one numeric column, regardless of null row values. Ambiguous choices are refused with a request to name y. The renderer never picks a replacement column. Proportional charts refuse unsafe tiny measurements and zero totals before drawing.
