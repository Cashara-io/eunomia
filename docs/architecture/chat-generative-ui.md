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

Unknown components, invalid props and missing bindings show an inline notice and the plain source answer. Chart code is loaded only when needed. Both themes use existing design tokens. Charts provide accessible names, summaries, keyboard and pointer tooltips, and legends for multiple series. Charts share one active tooltip owner. Focusing or pointing at another chart replaces the previous tooltip. Blur, pointer leave, pointer cancellation and unmount dismiss only the owning chart's tooltip, so a late event from an old owner cannot close the current tooltip. Each chart renders at most one tooltip node. Recharts remains in `frontend/src/app/(app)/settings/UsageSection.tsx`; chat has no Recharts imports.

## Model prose and resolved chart columns

Model titles, captions, labels, legends, column headers, tab and section names and notes use bound placeholders for numeric claims. For example, `Total {{sources.sales.rows[0].amount}}` or `Total {{/sources/sales/totals/amount/sum}}` binds one exact path in the same `/sources/...` state as numeric props. Dotted paths are a convenience spelling for pointers. Totals use the returned column key and an operation. For example, a dataset aggregated with sum of amount has `/totals/sum_amount/sum`, while a detailed dataset has `/totals/amount/sum`. Histograms, scatter plots and detail tables omit aggregates because the server already returns totals and distributions. Use JSON pointer escapes `~0` and `~1` for keys containing tildes or slashes. The server requires a non-null, server-typed numeric or date value. It rejects missing paths, text columns and untyped numeric fields with exact valid numeric placeholder paths from the resolved sources, so a model can repair its call. Values use the shared number formatter and the bound column or metric's unit and currency. Counts remain unitless. Date values retain their server ISO spelling.

Validation and display use the normalised complete final substituted string. It applies NFKC first, removes all Unicode format and nonspacing or enclosing combining marks, maps decimal digits to ASCII, maps dash and minus look-alikes to `-`, and maps Unicode whitespace to a plain space and collapses runs of spaces to match browser display. Leading and trailing separator spaces are retained when mapping substitution offsets. Matching is case-insensitive. Returned UI prose and server summary prose use this normalised spelling, preserving capitalisation. The original string is never returned as prose. Explicit bidi controls are removed, including embeddings, overrides, isolates and direction marks. Arabic and Hebrew letters without digits still pass. A known limitation is that legitimate RTL content needing explicit direction marks, or nonspacing diacritics, loses those marks. Eligible server strings use exactly the same normalisation and numeric-span extraction.

The server extracts numeric spans greedily. Signs, currency symbols and codes, percent and per-mille signs, exponent markers and fraction slashes travel with their digits. Whitespace, including any Unicode space after normalisation, cannot detach a sign, percent, per-mille, currency symbol or currency code from its digits. Thus `− 12`, `12 %` and `12 €` require one whole matching source output. `per cent` remains ordinary words. NFKC exposes vulgar fractions. A decimal or thousands separator, including `.`, `,`, apostrophe, underscore or one space, joins the span only when a digit follows it directly. Thus `12,345`, `12 345`, `12.5` and `12e3` are complete spans. Sentence punctuation in `Scope 3.`, `{{count}}.` and `{{count}}, then` stays outside the span. Words between figures separate them.

Every span must have exactly one complete source. A placeholder records its substitution range. Its complete formatted output must equal the span at those same normalised offsets. A span overlapping multiple placeholders, or a placeholder plus any extra numeric connector, is refused. Otherwise, a raw span must equal a complete span inside one eligible server string, with its letter-bearing phrase matched verbatim at those offsets. Permissions from different strings cannot combine. ISO dates and ranges form complete spans too, and every endpoint must match an applied date parameter or a returned bucket label. Parts of dates and query limits cannot authorise raw quantities.

The letter-bearing phrase rule still requires an adjacent letter-bearing word or an alphanumeric word from that single server string. Bare numeric strings such as `12` or `12%` never authorise digits. The allowlist includes entity names, kind and property names, column labels and text category values. It excludes model titles, headlines, affected-entity explanations, generated summaries, numeric values and arbitrary query arguments. Thus `ISO 27001 compliance`, `Scope 3 emissions` and `Q3 2026` need matching server phrases or bucket labels. They have no identifier exemption.

Number words such as "twice", "twelve" and "dos mil" remain allowed and unchecked. This guard does not establish provenance for words or the surrounding claim. An exact placeholder establishes where the displayed value came from, not whether the model's sentence is a sound interpretation.

Eligible server strings must have been created before the current request or chat turn began. Records updated since that cutoff are also excluded. Chat scopes one fixed turn-start timestamp around the complete model/tool loop and passes it into view resolution. The server intersects resolved strings with persisted strings in vaults the caller can read, using that cutoff. Names planted in earlier turns remain user-visible data and may qualify as phrases. This residual risk means a matched name is evidence of a stored spelling, not independent evidence for the model's claim. The server resolves UI props and legacy source headlines and explanations, then returns the substituted spec and text summary. MCP and Claude Code receive the same formatted text. Every UI prose block, including Text and Callout, displays the returned string literally as React text. There is no Markdown, HTML entity, backslash or Unicode-escape decoding after validation. JSON escapes decode once at the request boundary, before validation. The same whole-span check resolves UI props and model prose used in terminal summaries. The returned text summary uses those already validated, substituted values. Terminal summaries separate prose values with bullet items, so figures from separate fields cannot join into a numeric span after whitespace normalisation. Server-generated rows and totals retain their own typed formatting.

The server returns explicit x, y and series columns for every chart. An omitted y is resolved only when server metadata identifies exactly one numeric column, regardless of null row values. Ambiguous choices are refused with a request to name y. The renderer never picks a replacement column. Proportional charts refuse unsafe tiny measurements and zero totals before drawing.

Source aliases and element IDs use ASCII letters and underscores, with digits allowed only when their complete numeric span attaches to letters, such as `sales_q3`. Bare or leading numeric runs are refused. Chart selectors, series and legend keys, table column keys and format-map keys use the same rule. Namespaced server column keys such as `link:supplied_by` validate each part. Server summaries label sources by their position, such as `Source 1`, rather than copying aliases. Element IDs are internal references. Tab IDs and visible tab positions are generated by the renderer, rather than written by the model. Prose labels still use the whole-span provenance check.
