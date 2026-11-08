# Eunomia 2.2.0 release gate: auto-ontology end to end

**Verdict: FAIL.** The engine is correct: every number checks out, nothing leaks from what-ifs, and strict mode holds. But two High defects block the owner's real flow. The in-app chat answers number and what-if questions from the wrong space. The owner cannot see an org-space CSV in Documents, so "Import rows" is unreachable from the menus.

- main SHA: `4fe6ab4dccc5c4e83d255015848ab552c405203e` ("Merge pull request #177 from Cashara-io/feat/ontology-coverage"); backend reports version 2.1.0
- Date: 8 Nov 2026
- Stack: fresh `surrealdb/surrealdb:v3.3.5` container `eunomia-e2e-fresh` on 127.0.0.1:8100 (same hardened flags as `docker-compose.yml`, documents bucket `file:/data/documents`), debug backend on 127.0.0.1:8481 (model gpt-6-luna), `next dev` on 127.0.0.1:3480, fresh admin, MCP token
- Demo org: "Northside Bikes", Brunswick. Raw files: `nb/parts.csv` (25 rows, `$1,249.00` style money, day-first dates, "Shimano Oceania" and "shimano oceania"), `nb/shop-notes.md`
- Paths below are relative to this scratch folder: `/private/tmp/claude-501/-Users-avi--t3-worktrees-eunomia-t3-30a5aa7f/4e4b4a2b-5e3c-4db3-834d-e1a2852d3403/scratchpad/`

## Step table

| # | Step | Result | Evidence |
|---|------|--------|----------|
| 0 | Fresh stack from main, empty DB (all data tables 0) | PASS | `ev/counts-start.txt` |
| 0a | First attempt: plain SurrealDB container without `--allow-experimental=files` | Setup error (mine), rebuilt from scratch | `p1-aborted-setup/` |
| 1.1 | Agent creates org space "Northside Bikes" | PASS | `p1/transcript.txt` |
| 1.2 | Uploads `parts.csv` and `shop-notes.md` | PASS | same |
| 1.3 | `ontology_automap` on the CSV: kind `product`, 6 typed properties (money AUD, integer, enum, date) | PASS | same |
| 1.4 | Automap supplier link | FAIL (D3) | same: link aimed at kind `supplier`, "0 of 5 match", dropped |
| 1.5 | Notes as memories with kinds (21 memories), 19 relations, spelling variant folded into one org with alias | PASS | same |
| 1.6 | Proposes ontology: 7 kinds, 18 properties, 19 link types, 9 metrics; approves nothing | PASS | same |
| 1.7 | Certified metrics for total stock value and gross margin | FAIL (D4) | the grammar has no per-row product or difference; agent invented derived columns import cannot fill |
| 2.1 | Owner: Kinds and links before | PASS | `shots/p2-01-kinds-before.png` |
| 2.2 | Owner approves 9 kinds, 15 properties, 5 metrics, 19 links; rejects 3 derived properties and 4 metrics that need them (55 clicks, all 200) | PASS | `ev/p2-decide-log.txt`, `shots/p2-02-kinds-after-decisions.png` |
| 2.3 | Reject a duplicate kind with a merge | N/A | no duplicate kind was proposed |
| 2.4 | Find `parts.csv` in Documents to import it | FAIL (D2) | `shots/p2-03a-documents-list-missing-org-csv.png` |
| 2.5 | "Import rows" (opened by direct link `/documents?id=...`): 25 created, 150 values, 0 bad | PASS | `shots/p2-04-import-rows-done.png`, `ev/p2-import-response.json` |
| 2.6 | Import makes `product supplied_by organisation` links | FAIL (D3) | supplier column "skipped: no property is mapped from it"; 0 `supplied_by` relations |
| 2.7 | Readiness panel | PASS (shows), see D8 | `shots/p2-05-readiness-panel.png` |
| 2.8 | Strict mode on | PASS | `shots/p2-07-strict-on.png`, API `strict: true` |
| 3.1 | Q1 average price and total stock value: `ontology_coverage` + `metric_eval`; $337.23; refuses stock value | PASS (see D6) | `shots/p3-q1-transcript.png` |
| 3.2 | Q2 gross margin on Shimano parts: cost is approved, so it should be computed | FAIL (D3, D4) | `shots/p3-q2-transcript.png`: honest refusal, no number guessed |
| 3.3 | Q3 Van 2 late: neighbourhood + impact `show_view`; SJ-1041, SJ-1046, CO-2215, three customers | PASS | `shots/p3-q3-transcript.png` |
| 3.4 | Q4 Shimano +8%: `scenario_run` + scenario `show_view`, says nothing saved; row counts unchanged | PASS (see D7) | `shots/p3-q4-transcript.png`, `ev/counts-before-q4.txt` = `ev/counts-after-q4.txt` |
| 3.5 | Q5 rent: coverage 0.0, refuses to make a number up | PASS | `shots/p3-q5-transcript.png` |
| 3.6 | Q6 e-scooters under new kind in strict mode: refused, proposal filed, no memory written | PASS | `shots/p3-q6-transcript.png`, `ev/counts-*-q6.txt`; direct check `ev/strict-direct-write.txt` shows the refusal files a proposal by itself |
| 4.1 | Chat Q1 (as asked) | FAIL (D1) | `shots/p4-c1-chat.png`: "doesn't yet cover parts" (checked the personal space) |
| 4.1b | Chat Q1 naming the space | PASS | `shots/p4-c1b-chat.png`: metric card $337.23 |
| 4.2 | Chat Q3 Van 2: impact card | PASS | `shots/p4-c3-chat.png` |
| 4.3 | Chat Q4 (as asked), attempt 1 | FAIL (D5) | `shots/p4-c4-chat-hung-attempt1.png`: hung over 4 min after the 7th tool call |
| 4.3b | Chat Q4 (as asked), attempt 2 | FAIL (D1) | `shots/p4-c4b-chat.png`: "no approved property covering prices" |
| 4.3c | Chat Q4 naming the space: scenario card says "Linked within 2 steps (not all are affected)", deltas add up, nothing saved | PASS | `shots/p4-c4c-chat.png`, `ev/counts-before-c4c.txt` vs after (only chat tables changed) |
| X1 | Every stated number vs `python3 -I` recompute (tolerance 0.01) | PASS (24 of 24) | `ev/numbers-check.txt`, `ev/recompute.txt` |
| X2 | Scenario wrote nothing (entity, entity_property, relation tables) | PASS | counts diff empty around Q4; around c4c only `chat_message`, `chat_thread`, `embed_cache` moved |
| X3 | Imported values, 5 sampled rows incl. `$1,249.00` and `03/09/2026`, plus all 25 | PASS (0 mismatches) | `ev/import-verify.txt` |

## Numbers

| Quantity | Stated | Recomputed from parts.csv | Result |
|---|---|---|---|
| Average part price | 337.228 (metric), $337.23 (Q1, chat) | 337.228 | match |
| Average cost | 235.166, $235.17 | 235.166 | match |
| Active / out of stock / discontinued | 19 / 4 / 2 | 19 / 4 / 2 | match |
| Total stock value | not given (no metric) | 21,332.55 at price; 14,062.25 at cost | correctly refused |
| Shimano gross margin | not given (refused) | 30.48% (sum of 7 rows); 33.10% stock-weighted | correctly refused, but should have been computable (D3, D4) |
| What-if avg cost after | 240.00424, $240.00 | 240.00424 | match |
| What-if avg cost change | 4.83824 (tool), $4.84 (chat prose), "up 4.83" (card) | 4.83824 | match (card subtracts shown figures, by design) |
| What-if change % | 2.06%, 2.1% | 2.057% | match |
| Extra cost, one of each Shimano part | about $121 | 120.956 | match |
| Groupset cost after / extra | 977.40 / 72.40 | 977.40 / 72.40 | match |
| Other 6 Shimano costs after | 214.65, 163.30, 89.10, 77.11, 58.32, 53.03 | 214.65, 163.296, 89.10, 77.112, 58.32, 53.028 | match |
| Sum of card's 7 changes / 25 | 120.96 / 25 = 4.838 | 4.838 | deltas add up |

## Defects

**D1. High. In-app chat checks the personal space for number and what-if questions.**
`ontology_get` and `ontology_coverage` called without `vault_id` fall back to the personal space. The chat has no space picker and gpt-6-luna does not pass one, so it wrongly says the data "doesn't cover parts" or "has no approved property covering prices". `recall` searches every space, so the chat knows the facts but refuses the numbers.
Repro: org space with approved `product.price` and metric `avg_price`. In Chat, ask "What is our average part price and total stock value?". `chat_message` shows `ontology_get {}` returning `vault:p6cc...` (Personal) and coverage 0.0. Adding "In the Northside Bikes space:" fixes it.
Evidence: `shots/p4-c1-chat.png`, `shots/p4-c4b-chat.png`, `ev/chat-messages.json`.

**D2. High. Documents page lists only personal-space documents.**
`useDocuments()` calls `GET /api/documents` with no `vault_id`, and the server defaults to the personal space. An agent-uploaded org CSV never appears, so the owner cannot reach "Import rows" from the menus. The CSV hint even says to go to Kinds and links, which has no import button. The panel opened by direct link shows `parts.csv` beside a list that says "0 documents".
Repro: upload a CSV into an org space over MCP, then open Documents. Code: `frontend/src/lib/queries/documents.ts:19-24`, `backend/src/routers/documents.rs:65-67`.
Evidence: `shots/p2-03a-documents-list-missing-org-csv.png`, `shots/p2-04-import-rows-done.png`.

**D3. Medium. Automap aims the supplier link at a kind that does not exist, so import never links parts to suppliers.**
Automap proposed `product supplied_by supplier` and checked it against kind `supplier` (none exist). It reported "0 of 5 match" and dropped it, even though all 4 suppliers existed as `organisation` entities. The agent then proposed `product supplied_by organisation`, and the owner approved it, but "Import rows" still skips the supplier column ("no property is mapped from it"). Result: 0 `supplied_by` relations. "Shimano parts" cannot be selected by filter or `related_to`, so Q2 cannot be computed and Q4 had to fall back to matching names.
Evidence: `p1/transcript.txt` (automap result), `ev/p2-import-response.json`, `p3/q4.jsonl` (first `scenario_run` matched 0 products).

**D4. Medium. The metric grammar cannot express total stock value or gross margin from raw columns.**
`MetricExpr` has sum, count, avg, min, max, count_distinct and ratio of two aggregates (`backend/src/ontology/metric.rs:36-48, 140-175`). It has no per-row product (price x stock) and no difference (price minus cost). The brief's "total stock value" and "gross margin" can only be built on derived properties that import cannot fill. The agent proposed `stock_value_cost`, `unit_margin` and similar, and even claimed "the import has to compute them". The owner had to reject them.
Evidence: `p1/transcript.txt`, `ev/ontology-before.json`.

**D5. Medium. The chat model call has no timeout.**
Attempt 1 of chat Q4 stopped after its 7th tool call (`entities_search` at 05:03:31 UTC). Nothing was persisted or logged after that, and the Stop button stayed up for over 4 minutes. `llm_net::client` builds a `reqwest` client with no request or read timeout (`backend/src/llm_net.rs:133-137`), and the stream loop waits forever (`backend/src/chat/service.rs:674`). A stalled upstream leaves the chat hung with no error. Attempt 2 returned in 10 s, so the stall is intermittent.
Evidence: `shots/p4-c4-chat-hung-attempt1.png`, `ev/chat-c4-tail.json`, `backend.log`.

**D6. Low. `ontology_coverage` overclaims.**
For "What is our average part price and total stock value?" it returned coverage 1.0, matching "total stock value" to property `stock` (distance 0.53), although no stock-value metric exists. The agent still refused correctly, but the score is misleading.
Evidence: `p3/q1.txt.out`.

**D7. Low. The scenario change label is garbled when `related_to` matches no relation.**
The label reads "Product cost up 8% on 0 products  Shimano Oceania" (double space, no verb). With zero edges, `edges.iter().all(...)` is vacuously true, so it takes the empty label of `edges.first()` (`backend/src/ontology/scenario.rs:336-340`). The warning beside it is clear.
Evidence: `p3/q4.jsonl`.

**D8. Low. Readiness never reaches "ready" for a normal business.**
Every approved kind must carry an approved metric, so built-ins like `person`, `organisation` and `vehicle` block it forever ("no approved property or metric on kind person").
Evidence: `shots/p2-05-readiness-panel.png`.

**D9. Low. The CSV profile labels money columns as "Text".**
`price AUD` and `cost AUD` show Type "Text" with values like `$1,099.0...`. Import still parsed them as money correctly.
Evidence: `shots/p2-04-import-rows-done.png`.

## Notes

- Test artefacts left in the space before teardown: a proposed kind `gift_card` (my direct strict-mode check) and `e_scooter` (Q6). The container is deleted at the end.
- Agent-side remarks (not product defects): the Q1 agent said approving a stock-value field would make the shop "work out stock x cost", which is not true (see D4). The Q2 agent called the margin metric's proposer note the owner's reason.
- Fixed by me during setup, not a product defect: the first container lacked the files feature, so uploads returned `document.storage_unavailable` (logged clearly by the backend). Rebuilt with the compose flags.
