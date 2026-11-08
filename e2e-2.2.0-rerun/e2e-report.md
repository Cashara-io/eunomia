# Eunomia 2.2.0 release gate, rerun: auto-ontology end to end

**Verdict: FAIL.** Four of the five old defects are fixed (Documents, auto-map links, row maths, timeout). The fifth is only half fixed: the chat still checks the personal space when a question's words match nothing. Three new High defects block the owner's flow: a "remember" with no space named skips strict mode and lands in the personal space, a margin "on one supplier's products" cannot be computed, and the gross margin metric is wrong when a row has a blank cost.

- main SHA: `2774b5dc` (`2774b5dc Merge pull request #179 from Cashara-io/feat/gate-links-and-row-maths`); backend reports 2.1.0
- Date: 8 Nov 2026
- Stack: fresh `surrealdb/surrealdb:v3.3.5` container `eunomia-e2e-fresh2` on 127.0.0.1:8100 (compose flags, documents bucket `file:/data/documents` bound to a fresh empty folder), debug backend on 127.0.0.1:8481 (gpt-6-luna, fresh JWT and encryption keys), `next dev` on 127.0.0.1:3480, fresh admin, MCP token
- Demo org: "Geelong Garden Supply", Geelong VIC. `ggs/products.csv` (30 rows, `$1,249.00` money, day-first dates, "Barwon Soil & Mulch" and "barwon soil & mulch", Turf Underlay with a blank cost, 4 rows with qty 0), `ggs/yard-notes.md` (6 trade customers and orders, Truck 4, a fee decision, a Coastal Irrigation rumour)
- Phase 3 questions named no space. Prefix used: "You are the AI assistant for our business. Use only the eunomia-local tools to answer. Question from the owner:"
- Paths below are relative to `/private/tmp/claude-501/-Users-avi--t3-worktrees-eunomia-t3-8ef11497/9dd1cb91-d4ed-487b-9b57-c193786da00e/scratchpad/`. Screenshots also on branch `pr-screenshots` under `e2e-2.2.0-rerun/`.

## Old defects

| Old defect | Status | Evidence |
|---|---|---|
| D1 chat reads only the personal space | **NOT FIXED (half)** | Fixed when a phrase matches an approved term: chat Q1 found Geelong Garden Supply by itself (`shots/p4-c1-chat.png`), MCP Q1 to Q3 too. Still broken when nothing matches: chat Q4 "What if Coastal Irrigation raises costs 10%?" got `ontology_coverage` from Personal (coverage 0.0) and answered "the Personal vault doesn't have an approved term for costs" (`shots/p4-c4-chat.png`, `ev/chat-messages.json`). See N2. |
| D2 Documents hides org-space files | **FIXED** | `products.csv` and `yard-notes.md` listed with a "Geelong Garden Supply" space chip, "All spaces" picker; clicking opens the panel and Import rows (`shots/p2-01-documents-list.png`, `shots/p2-02-products-csv-opened.png`) |
| D3 auto-map link aimed at a missing kind | **FIXED** | Auto-map proposed kind `supplier` together with `product supplied_by supplier` ("no supplier exists yet: an import creates the 4 named in supplier once each"). Import: 30 rows linked, 4 suppliers created, Barwon 7 rows incl. the lower-case spelling, one entity, no aliases needed (`p1/transcript.txt`, `ev/p2-import-response.json`). Caveat N6. |
| D4 no per-row maths | **FIXED (grammar)** | Agent proposed `sum(qty_on_hand x unit_cost)`, `avg(sell_price - unit_cost)`, a ratio of row sums; all accepted and run. Stock value at cost $24,743.70 correct. But the ratio form is wrong with blank cells (N4), and no metric can be split by supplier (N3). |
| D5 chat model call has no timeout | **FIXED** | Normal answers took 10 to 19 s, nothing hung. Backend restarted with `EUNOMIA_CHAT_MODEL_TIMEOUT_SECS=1`: turn ended after 3 s with "The model did not answer within 1 second, so this reply was stopped" and Retry (`shots/p4-c5-timeout-1s-error.png`, `p4/timeout-test.txt`) |
| Old low D8 (person blocks readiness), D9 (money shown as Text) | FIXED | Readiness no longer blocked by person or supplier; CSV profile says "Money" (`shots/p2-08-readiness-panel.png`, `shots/p2-02-products-csv-opened.png`) |

## Step table

| # | Step | Result | Evidence |
|---|------|--------|----------|
| 0 | Fresh stack from main, empty DB (only migrations, 1 personal vault) | PASS | `ev/counts-start.txt` |
| 1.1 | Agent creates org space, uploads both files | PASS | `p1/transcript.txt`, `shots/p1-transcript.png` |
| 1.2 | `ontology_automap`: kind product, 6 typed properties (money AUD, integer, enum, date), kind supplier + link | PASS | same |
| 1.3 | Notes as 28 memories under customer, order, shipment, decision, rumour, person, organisation | PASS | same |
| 1.4 | Proposes 6 more kinds, 7 properties, 8 link types, 8 metrics incl. row maths; approves nothing | PASS | same |
| 1.5 | "Link things": 0 business links made (agent says link types need approval first) | FAIL (N8) | same; `ev/counts-after-p1.txt` (only 16 extractor relations) |
| 2.1 | Owner finds `products.csv` on Documents | PASS (D2 fixed) | `shots/p2-01-documents-list.png`, `shots/p2-02-products-csv-opened.png` |
| 2.2 | Kinds and links before | PASS | `shots/p2-03-kinds-before.png`, `ev/p2-before.txt` |
| 2.3 | Approve 8 kinds, 13 properties, 10 metrics, 9 links (40 clicks, all 200) | PASS | `ev/p2-decide-log.txt`, `shots/p2-04-kinds-after-decisions.png` |
| 2.4 | Supplier link targets a kind that was proposed with it | PASS (D3 fixed) | `ev/ontology-before.json` |
| 2.5 | Reject a duplicate with a merge: no duplicate kind flagged; duplicate metric `inventory_cost` (same sum as `total_stock_value_at_cost`) rejected and merged in the UI, now "Runs total stock value at cost instead" | PASS | `shots/p2-05-duplicate-metric-merged.png` |
| 2.6 | Import rows: 30 new, 179 values (30 x 6 minus the blank cost), 0 bad, 394 ms | PASS | `shots/p2-07-import-rows-done.png`, `ev/p2-import-response.json` |
| 2.7 | Spelling variants one entity; every product linked to its supplier (8/7/8/7) | PASS | SQL in session; `ev/counts-after-import.txt` |
| 2.8 | Readiness | PASS (shows; blocked only by `units_on_open_orders`, which has no order lines) | `shots/p2-08-readiness-panel.png` |
| 2.9 | Strict mode on | PASS | `shots/p2-10-strict-on.png` |
| 3.1 | Q1 average sell price and stock value at cost: coverage found the org space by itself, metric_eval: $137.17, $24,743.70, notes Turf Underlay skipped | PASS | `shots/p3-q1-transcript.png` |
| 3.2 | Q2 gross margin on Barwon products: refused, "can't split margin by supplier"; quotes the all-products 44.1% | FAIL (N3, N4) | `shots/p3-q2-transcript.png` |
| 3.3 | Q3 Truck 4 late: neighbourhood used, names GGS-5012/5017/5023/5026 from facts; no impact view ("no links") | PARTIAL (N8) | `shots/p3-q3-transcript.png` |
| 3.4 | Q4 Coastal +10%: `scenario_run` with `related_to` supplier, 7 products, scenario `show_view`, says nothing saved; entity, entity_property, relates_to unchanged | PASS (margin figures wrong, N4) | `shots/p3-q4-transcript.png`, `ev/counts-before-q4.txt` vs `ev/counts-after-q4.txt` (only embed_cache moved) |
| 3.5 | Q5 electricity bill: coverage 0.0, refuses to guess | PASS | `shots/p3-q5-transcript.png` |
| 3.6 | Q6 chickens under new kind, strict on: saved under kind `livestock` in the **Personal** space, no refusal, no proposal | FAIL (N1) | `shots/p3-q6-transcript.png`, `ev/counts-before-q6.txt` vs after (entity +1, memory +1) |
| 3.6b | Same write naming the space: refused, filed as proposal, no entity | PASS | `ev/strict-direct-write.txt` |
| 4.1 | Chat Q1, no space picked: two metric cards, $137.17 and $24,743.70, 16 s | PASS | `shots/p4-c1-chat.png`, `shots/p4-c1-chat-full.png` |
| 4.2 | Chat Q3: impact card, 10 s; card says "Nothing else is affected." while the text names 4 orders | FAIL (N7, N8) | `shots/p4-c3-chat.png` |
| 4.3 | Chat Q4 as asked: checked Personal, refused, 10 s | FAIL (D1, N2) | `shots/p4-c4-chat.png` |
| 4.3b | Chat Q4 reworded "unit cost of its products": scenario card, 7 changes, deltas add up, "Nothing was saved", 19 s; no "Linked within N steps" line because `reached` is empty (no links past the supplier) | PASS (card shows margin as "0.44 percent", N5) | `shots/p4-c4b-chat-full.png`, `ev/counts-before-c4b.txt` vs after (only chat tables and embed_cache) |
| 4.4 | Nothing hangs; timeout fires at the set limit | PASS | `ev/chat-timings.txt`, `shots/p4-c5-timeout-1s-error.png` |
| X1 | Stated numbers vs `python3 -I` recompute | FAIL (17 of 19; both misses are N4) | `ev/numbers-check.txt`, `ev/recompute.txt` |
| X2 | Scenarios wrote nothing (entity, entity_property, relates_to) | PASS | counts files above |
| X3 | Imported values: 5 rows incl. `$1,249.00` pump, blank-cost Turf Underlay, day-first dates; plus all 30 | PASS (0 mismatches) | `ev/import-verify.txt` |

## Numbers

| Quantity | Stated | Recomputed from products.csv | Result |
|---|---|---|---|
| Average sell price | 137.173333, $137.17 | 137.173333 | match |
| Total stock value at cost | 24,743.70 | 24,743.70 (29 rows) | match |
| Total stock value at retail | 45,990.30 | 45,990.30 | match |
| Average unit margin | 53.918966 | 53.918966 | match |
| Back-ordered / product count / units | 4 / 30 / 1,215 | 4 / 30 / 1,215 | match |
| Discontinued stock value at cost | 1,039.50 | 1,039.50 | match |
| Gross margin (stock-weighted, the approved metric) | 0.440584, 44.1%, 44.06% | **45.02%** (costed rows) | **WRONG** (N4: denominator keeps the blank-cost row: 20,262.60 / 45,990.30) |
| Gross margin by the brief's rule sum(p-c)/sum(p) | not offered | 38.77% | n/a |
| Barwon Soil & Mulch margin | not given (N3) | 40.71% simple, 41.80% stock-weighted | not computable |
| What-if avg unit margin | 53.92 to 50.02, down 3.90 (7.2%) | 50.0207, 3.8990, 7.23% | match |
| What-if stock value at cost | 25,511.00, up 767.30 (3.1%) | 25,511.00, 767.30, 3.10% | match |
| What-if gross margin after | 42.39% (0.42) | **43.32%** | **WRONG** (N4) |
| What-if pump / controller cost | 893.20 / 195.80 | 893.20 / 195.80 | match |
| Card's 7 cost changes sum / 29 | 113.07 / 29 = 3.899 | 3.899 | deltas add up |

## New defects

**N1. High. A write with no space named goes to the personal space, so strict mode never sees it.**
Repro: org space with strict on, user also has a personal space. Over MCP ask "Remember that we now sell chickens. Save it under a new kind called livestock." The agent calls `entity_types {}` (which lists the org space's approved kinds) and then `memory_write` with no `vault_id`. The write lands in Personal under a new kind `livestock`; nothing is refused and no proposal is filed. The same write with `vault_id: "Geelong Garden Supply"` is refused and filed (`ev/strict-direct-write.txt`). A business fact ends up private to one person, and the owner's strict mode is sidestepped without anyone noticing.
Evidence: `shots/p3-q6-transcript.png`, `p3/q6.jsonl`, `ev/counts-before-q6.txt`, `ev/counts-after-q6.txt`.

**N2. High. When no phrase matches any space, `ontology_coverage` answers from the personal space (the rest of D1).**
`scope::readable` puts the default vault first, so a 0.0 tie picks Personal. "costs" does not match `unit_cost` anywhere, so chat Q4 says "the Personal vault doesn't have an approved term for costs" and stops. MCP Q4 and Q5 got Personal first too; Claude retried with the id, gpt-6-luna did not.
Repro: the setup above; in Chat ask "What if Coastal Irrigation raises costs 10%?".
Evidence: `shots/p4-c4-chat.png`, `ev/chat-messages.json` (11:39:51), `p3/q4.jsonl`, `p3/q5.jsonl`. Code: `backend/src/ontology/scope.rs` (`readable`, sort by default first).

**N3. High. `metric_eval` cannot narrow a metric by a link, so "margin on Barwon products" is unanswerable.**
Every product is linked `supplied_by` its supplier, and `scenario_run` accepts `related_to`, but `metric_eval` takes only property `filters` (no `related_to`, no `entity_ids`; `EvalArgs` in `backend/src/ontology/metric.rs:1124`). The agent correctly refused and suggested a supplier property, which import never fills (the column maps to the link).
Repro: after import, ask "What is our gross margin on Barwon Soil & Mulch products?".
Evidence: `shots/p3-q2-transcript.png`.

**N4. Medium. A ratio of row sums drops a row from one side only, so gross margin is wrong when a cell is blank.**
`gross_margin` = sum((price - cost) x qty) / sum(price x qty). The numerator skips Turf Underlay (blank cost), the denominator keeps it: 20,262.60 / 45,990.30 = 44.06% instead of 45.02%. `n_entities` says 30, `warnings` is empty. The what-if margin is off the same way (42.39% vs 43.32%). The #179 test fixture has no blank costs, so it was not caught.
Repro: approve `gross_margin` as above on `products.csv`; `metric_eval gross_margin`.
Evidence: `ev/metric-gross-margin-full.json`, `ev/numbers-check.txt`.

**N5. Medium. Percent metrics show a fraction labelled "percent".**
The scenario card shows gross margin "0.44 -> 0.42 percent" and "down 0.02 percent (3.8%)". The model's prose says 44.06% to 42.39%. An owner reads "0.44 percent margin".
Evidence: `shots/p4-c4b-chat-full.png`.

**N6. Medium. Import creates supplier entities next to same-named organisations already in the space.**
Document indexing had already made organisations "Barwon Soil & Mulch", "Coastal Irrigation Co", "Australian Native Nurseries", "Bunnings Trade". Auto-map checked only kind `supplier` ("matched 0") and import created 4 new `supplier` entities with the same names, no merge or warning. Facts split: the price-rise rumour sits on organisation "Coastal Irrigation Co", the 7 products link to supplier "Coastal Irrigation Co"; `entities_search` returns both. The 6 customers are doubled the same way (organisation from the notes, customer from the agent).
Evidence: `p1/transcript.txt` (automap shows organisation with 4 entities), `p3/q4.jsonl` (entities_search), SQL listing in session.

**N7. Medium. The impact card says "Nothing else is affected." when it simply has no links to follow.**
Chat Q3: the card under "Truck 4 is a day late." says "Nothing else is affected.", and the text below names 4 orders that may be affected. The card should say there are no saved links, not that nothing is affected.
Evidence: `shots/p4-c3-chat.png`.

**N8. Low. There is no business-link tool, only `code_relate` ("symbol calls symbol").**
The docs say business relations go through `code_relate`, but its name and description are about code. This run's mapping agent made 0 business links ("link types need approval first"); the first gate's agent used it 19 times. Result: Truck 4 has no links to its orders, Q3 has no impact trace, and every scenario's `reached` is empty, so the "Linked within N steps" line could not be checked.
Evidence: `p1/transcript.txt`, `shots/p3-q3-transcript.png`.

**N9. Low. UI text leaks raw ids and snake_case.** The merged metric row reads "computed from the document:pp2zxssp1mluaa4wvr8w records' properties" (`shots/p2-05-duplicate-metric-merged.png`). Readiness says "metric units_on_open_orders on order_line uses quantity" (`shots/p2-08-readiness-panel.png`).

**N10. Low. CSV profile clips the link label.** "-> supplied by su" runs into the Type column (`shots/p2-07-import-rows-done.png`).

**N11. Low. `ontology_coverage` misses plain words for approved terms.** "costs" in the right space gives coverage 0.0, though `unit_cost` is approved (`p3/q4.jsonl`). This feeds N2.

## Notes

- Test artefacts left before teardown: Personal entity "Chickens" (kind livestock, from Q6), proposed kind `poultry` (my direct strict check). The container is deleted at the end.
- Agent remarks, not product defects: the Q2 agent suggested adding a supplier property; the Q4 MCP agent's "margin drops about 3.8%" is relative to the wrong margins (N4).
