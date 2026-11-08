# Eunomia 2.2.0 release gate 4

**Verdict: FAIL.** PR #189 fixes four requested gate 3 cases on the new business. Money rounding is partly fixed, with a remaining average-cost display inconsistency. Ten of eleven gate 2 items are fixed. N9 remains in saved-chat activity summaries. Three numeric precision failures, a money-display inconsistency and an agent's wrong-kind write fail the requested end-to-end acceptance. No High arithmetic or import defect was reproduced. No product code was changed.

## Stack and evidence

Main SHA: 6705227734c84c6c4164811476bdd1dfc39efea4, detached from freshly fetched origin/main. Date: 9 November 2026, Australia/Melbourne. Database timestamps are 8 November UTC. Repository origin was verified live as Cashara-io/eunomia.

Initial disk: 19 GiB free, above 12 GB. Backend build passed in 2m 49s, using only the supplied shared cargo-lock.sh. No cargo fmt. bun install passed. Backend package still identifies itself as 2.1.0, a release-version observation.

Fresh in-memory surrealdb/surrealdb:v3.3.5 container eunomia-e2e-fresh4, on 127.0.0.1:8100. Compose deny-all, allowlists and timeouts retained. Fresh host documents folder mounted at /data/documents, with EUNOMIA_DOCUMENTS_BACKEND=file:/data/documents. Backend on 127.0.0.1:8481, Next dev on 127.0.0.1:3480. Real in-app gpt-6-luna, unchanged. A final timeout probe only changes the deadline to one second.

First admin registered and onboarded using the same endpoints as scripts/ci/e2e-setup.sh. Fresh password and runtime keys. Credentials and MCP configuration are mode 600 in a mode 700 scratch folder. None are published.

Fictional Fremantle Fish & Ice Co, Fremantle WA: 30 raw CSV rows, four suppliers, Southern Ice Works spelling variant, two blank costs, one zero-stock row, $1,180.00, $7.45, day-first dates and three statuses. Before mapping began, one quantity was adjusted so the aggregate scenario also ends on a half cent. Notes have six customers and open orders, Reefer 2, inbound load IN-42, a $250 delivery minimum from 1 December 2026, an unconfirmed price rumour, existing supplier organisations and unlinked Office Label Printer. Nothing was pre-mapped.

Three memberships verified: Personal, Freo Rowing Club and Fremantle. Rowing has three unrelated topics. Personal is empty.

Evidence paths below are relative to /private/tmp/eunomia-gate4/. Published files are under e2e-2.2.0-gate4/ on pr-screenshots. Raw transcripts are unchanged. Transcript PNGs render actual final text and call summaries; complete tool inputs and results remain in JSONL, tools.json and expandable HTML. Browser evidence uses the shared Playwright preview. Capture limits are disclosed in ev/screenshot-note.txt.

## Step results

| Step | Result | Evidence |
|---|---|---|
| Fresh main, disk, locked build, bun install | PASS | ev/main-sha.txt, ev/build.log, ev/bun-install.log |
| Fresh DB, documents, admin and token | PASS | ev/container.txt, ev/counts-start.json, ev/vaults.json |
| Mapping agent, MCP only | PASS | p1/transcript.jsonl and tools.json. Every called tool is eunomia-local |
| Notes before CSV, supplier organisations and business links | PASS | p1/transcript.tools.json, ev/preimport-organisations.json. Forty-two entity_relate calls, four rejected bad IDs corrected, 38 saved links |
| Auto-map supplier direction | FAIL, Low | Product supplies organisation proposal reads backwards. Agent proposed supplied_by; owner rejected backwards link. F4 |
| Owner browser review and decisions | PASS | shots/p2-kinds-before.png, p2-kinds-approved.png; ev/ontology-before.json and ontology-approved.json. Ten kinds, eight properties, six metrics, eleven links approved. Duplicate inventory_cost_value and backwards link rejected |
| Browser CSV import | PASS | ev/import-ui.json, shots/p2-import-done.png. Thirty rows: 15 new, 15 updated, 178 values, 30 links, zero supplier targets created |
| Spelling variants and existing suppliers reused | PASS | ev/import-verify.json. All 30 supplier links checked, four pre-existing supplier IDs retained, zero duplicate supplier/customer names |
| Mapping labels and readiness | PASS, separate list-layout issue F6 | Ellipsis and full tooltip, no Type-column spill. Ready at 10 kinds, 8 properties, 6 metrics, 11 links. shots/p2-csv-approved.png and p2-readiness.png |
| Owner enables strict mode | PASS | shots/p2-strict-on.png, ev/ontology-final.json |
| MCP Q1, no space named | PASS | $130.79 average, $27,424.50 stock at cost. p3/q1.tools.json and txt |
| MCP Q2 supplier margin | FAIL, presentation | Correct related_to result 35.680333%, final says 35.7%. p3/q2.tools.json and txt, F2 |
| MCP Q3 delay | PASS links, FAIL model reassurance | Four correct linked orders, but two called safe without allocation/capacity evidence. p3/q3.txt, F7 |
| MCP Q4, costs +10% | PASS | Correct scenario, readable warning, no writes, coverage 1.0 without raises. p3/q4.tools.json and txt |
| MCP Q5 electricity bill | PASS | Refuses to invent, uses coverage and recall. p3/q5.txt |
| MCP strict write, no space named | PASS | Refusal names all three spaces. Retry in Fremantle refused with proposal. No entity or memory added. p3/q6.tools.json, ev/strict-no-vault-direct.json |
| MCP named-space strict write | FAIL, agent behaviour | Saves organisation note first, then live_seafood refused/proposed. p3/q6-named.tools.json, F3 |
| Bare Reefer 2 and Esperance questions | PASS | Correct Fremantle entities, never Personal. p3/q7 and q8, ev/direct-coverage-0.json and -1.json |
| In-app Q1, no space selected | PASS data, N9/F5 presentation issues | Correct cards. shots/p4-q1.png and p4-q1-full.png, p4/q1.txt |
| In-app Q3, no space selected | PASS | Truck, load and four orders/products. shots/p4-q3.png, p4/q3.txt |
| In-app Q4, no space selected | FAIL relative-percent precision | Absolute deltas pass G1. Two relative percentages exceed tolerance. shots/p4-q4-top.png and p4-q4-bottom.png, p4/q4.txt, F1 |
| In-app bare Esperance question | PASS | Correct supplier, product, customer, order and inbound load. shots/p4-bare-name.png, p4/bare-name.txt |
| Unlinked printer impact | PASS | Card and prose say no saved links and cannot identify downstream effects. shots/p4-unlinked.png, p4/unlinked.txt |
| Normal answers and forced timeout | PASS | Five normal answers complete. One-second deadline gives explicit stopped/retry message. shots/p4-timeout.png, p4/timeout.txt, ev/chat-with-timeout.json |
| Independent numeric audit | FAIL presentation | 23/23 structured checks pass; 72/76 other numeric checks pass. Three precision failures and one exact money-consistency failure below |
| Scenarios save nothing | PASS | MCP Q4 unchanged: entity 50, property 178, relation 68, memory 81. App unchanged at 50,178,68,82. Extra memory is F3. Comprehensive 12-table direct probe also unchanged, ev/counts-comprehensive-scenario-before.json and after.json |
| Imported rows and isolation | PASS | All 30 exact. Five samples include $1,180.00, 04/11/2026, blank cost and zero stock. Personal empty, Rowing only its three topics. ev/import-verify.json |
| Publication and teardown | Final receipts | ev/upload.json, ev/cleanup.json |

## Gate 2 items

FIXED means the original mechanism was retested successfully. Separate model-output failures still count against the end-to-end verdict.

| Item | Status | Evidence and limits |
|---|---|---|
| N1 no-space strict bypass | FIXED | Direct and agent writes without vault_id refuse and name Freo Rowing Club, Fremantle and Personal. Named live_seafood returns ontology.rejected. Zero live_seafood entities. p3/q6.tools.json, ev/strict-no-vault-direct.json. F3 is a different, approved-kind agent write |
| N2 unmatched questions select Personal | FIXED | Q4 uses Fremantle. Both bare-name direct probes have no data terms, coverage 0, matched/uncovered empty, chosen_by entities, vault_name Fremantle. MCP and app answers use the business. ev/direct-coverage-0.json and -1.json |
| N3 cannot filter metric by supplier | FIXED | related_to supplied_by Shark Bay Prawns selects eight products, seven eligible, value 35.680333%. p3/q2.tools.json |
| N4 different ratio rows | FIXED | Approved sum(sell_price - unit_cost) / sum(sell_price), percent. Both sides exclude the same two blanks, one for Shark Bay. Independent overall 35.761419952596%, supplier 35.680332739156%, scenario 35.644796380090%. ev/numbers-check.json |
| N5 fraction labelled percent | FIXED | Scaled API values and UI 35.76 to 35.64%, not 0.36 percent. shots/p4-q4-top.png |
| N6 duplicate suppliers/customers | FIXED | Zero targets created, four existing organisations reused, zero repeated supplier/customer names across kinds. ev/preimport-organisations.json, import-verify.json |
| N7 empty impact says nothing affected | FIXED for card | Printer card and gpt-6-luna prose explicitly say no saved links and unknown downstream effects. shots/p4-unlinked.png. F7 is separate headless-agent prose |
| N8 no business-link tool | FIXED | entity_relate used 42 times, four rejected IDs corrected, 38 saved links. Real truck/order/product paths. p1/transcript.tools.json, shots/p4-q3.png |
| N9 raw identifiers and snake_case in UI | NOT FIXED, narrower residual | Readiness, mapping document reasons, strict proposal and scenario warning readable. Reopening Q1 shows avg_sell_price and total_stock_value_at_cost in saved activity summaries. shots/p4-q1-full.png |
| N10 CSV mapping label spills into Type | FIXED | Ellipsis and full title tooltip, cell scrollWidth equals width 101. ev/import-ui.json, shots/p2-csv-approved.png. F6 concerns document filenames |
| N11 costs misses unit_cost | FIXED | Exact Q4 matches costs to approved product unit_cost, distance 0, coverage 1. p3/q4.tools.json, ev/direct-coverage-2.json |

## Gate 3 and PR #189 cases

| Case | Status | Evidence |
|---|---|---|
| G1 exact absolute delta and adding-up precision | FIXED | UI avg cost 85.180 to 85.335, +0.155; margin 35.76 to 35.64, -0.12 points; stock 27424.50 to 27639.88, +215.38. Displayed endpoints subtract exactly to displayed deltas. Each delta is within 0.01 of truth. MCP exact deltas 0.154643, -0.116624, 215.375. p4/q4.txt, ev/display-number-check.json |
| G2 raises falsely missing | FIXED | Exact Q4 coverage costs only, coverage 1, uncovered empty, no raises proposal. p3/q4.tools.json, ev/direct-coverage-2.json |
| N2 zero-phrase residual | FIXED | Both exact bare-name probes select Fremantle by entities despite empty matched/uncovered. ev/direct-coverage-0.json and -1.json |
| N9 scenario warning residual | FIXED | Warning starts gross margin, not gross_margin, in app and MCP. p3/q4.tools.json, shots/p4-q4-bottom.png. Broader N9 remains above |
| Half-cent money mismatch | NOT FIXED, partial | Exact total 27639.875 and delta 215.375 become 27639.88 and 215.38 in app and MCP prose. Block Ice 7.975 becomes 7.98 in both. Carton 7.425 becomes 7.43 in app. But average cost 85.335 stays 85.335 in app and becomes 85.34 in MCP prose. The requested identical-display rule fails, F8. p3/q4.txt, p4/q4.txt, ev/display-number-check.json |

## Independent calculations

Used python3 -I, csv and Decimal on the raw CSV and exact expression in ev/ontology-approved.json. Gross margin is the approved unweighted ratio of unit-price sums. Two blank-cost rows are out of both sides. Thirty have sell prices; twenty-eight have costs. Shark Bay has eight selected and seven eligible.

| Quantity | Independent |
|---|---:|
| Average sell price | 130.793333333333 AUD |
| Average unit cost | 85.180357142857 AUD |
| Stock at cost | 27,424.50 AUD |
| Stock at sell price, all rows | 46,227.40 AUD |
| Overall gross margin | 35.761419952596% |
| Shark Bay stock at cost | 11,621.25 AUD |
| Shark Bay gross margin | 35.680332739156% |
| Available products / all products | 26 / 30 |
| Southern Ice changed products | 8 |
| Scenario average cost | 85.335 AUD |
| Exact average-cost delta | +0.154642857143 AUD |
| Scenario margin | 35.644796380090% |
| Exact margin delta | -0.116623572506 points |
| Scenario stock cost | 27,639.875 AUD |
| Exact stock-cost delta | +215.375 AUD |
| Average-cost relative change | 0.181547556655% |
| Stock-cost relative change | 0.785337927765% |

Supplier unit-cost deltas are 0.725, 0.51, 0.32, 0.38, 1.30, 0.24, 0.675 and 0.18 AUD. Sum 4.33, divided by 28 gives the average-cost delta. Quantity weighting gives 215.375 AUD.

Five samples: Ice Brick 500g, Greenlip Abalone 1kg, Fish Frames 10kg, Dhufish Whole 10kg, Octopus 5kg. Seed 220, plus required Dhufish money/date and blank-cost Octopus. All 30 rows were checked, not just samples. Full field-by-field expected/actual values, supplier links and memberships are in ev/import-verify.json. Notes quantities, dates and linked order numbers were checked against the original ops-notes.md and complete DB export.

## Remaining and new defects

### N9 residual, Low: saved activity summaries expose internal metric names

Repro: finish Q1, open another conversation, reopen Q1. Cards use readable names, while ordinary visible activity summaries show avg_sell_price and total_stock_value_at_cost beside Worked out a number and Showed a view. This does not require opening raw JSON. Evidence: shots/p4-q1-full.png, ev/chat-final.json.

### F1, Medium: relative scenario percentages exceed 0.01 tolerance

Repro: exact Q4 in browser. Average-cost card says 0.2%, truth 0.181547556655%, error 0.018452443345 points. Stock-cost card says 0.8%, truth 0.785337927765%, error 0.014662072235 points. Structured values 0.18 and 0.79 pass; the card rounds further. Absolute deltas pass G1. Evidence: shots/p4-q4-top.png, p4/q4.txt, ev/display-number-check.json. Presentation failure, not wrong scenario arithmetic.

### F2, Medium: MCP supplier-margin prose rounds outside tolerance

Repro: new headless Q2 with no space named. metric_eval returns 35.680333%, independent truth 35.680332739156%. Final says 35.7%, error 0.019667260844 points. It needs at least 35.68% for this gate. Evidence: p3/q2.txt and tools.json, shots/p3-q2-transcript.png. Headless-agent presentation, not a metric error.

### F3, Medium: named strict-write agent saves under a different kind first

Repro: strict on, exact named-space live crayfish question. First memory_write targets organisation Fremantle Fish & Ice Co with the crayfish fact. It succeeds, creating organisation:nr42lg236k4p3w899wov and memory:rmfdjljh39si0t7yg9gf. Next live_seafood write is refused and proposed. Memory count 81 to 82; live_seafood has zero entities. Final answer admits misfiling and leaves it saved. The no-vault guard works; strict mode enforces kinds. This is agent instruction-following failure, not acceptance of an unapproved kind. Evidence: p3/q6-named.tools.json and txt, ev/counts-before-q6-named.json and after-q6-named.json, ev/ontology-final.json, shots/p3-q6-named-transcript.png. No write was removed to hide it.

### F4, Low: auto-map proposes a semantically backwards supplier link

Repro: auto-map raw CSV after organisations exist. Proposal product supplies organisation has description Organisation that supplies this product. Agent notices and proposes supplied_by from product to organisation. Owner rejects backwards proposal and approves replacement, so imported links are correct. Evidence: p1/transcript.tools.json, ev/ontology-before.json, p1/transcript.txt. One real-model reproduction, model-dependent.

### F5, Low: prose and cards disagree on as-of day

Repro: initial app Q1 at 06:30 Melbourne, 9 November. Cards say As of 9 Nov 2026. Prose says As of 8 November 2026. Tool timestamp is 8 November UTC. Correct data, two dates without timezone explanation. Evidence: p4/q1.txt, shots/p4-q1-full.png, ev/chat-final.json.

### F6, Low: document panel squeezes filenames to one letter

Repro: Documents at 1280 by 800, catalogue.csv open. Name column 57 CSS pixels, with icon/padding, leaving visible c and o for catalogue.csv and ops-notes.md. Panel shows selected full filename and closing restores space, but other list filenames are unusable. The mapped-label column is fixed under N10. Evidence: shots/p2-csv-approved.png, ev/document-layout.json.

### F7, Low: headless impact prose overstates safety from stock and absent links

Repro: MCP Q3. Says FFI-102 and FFI-104 look safe because catalogue stock exceeds quantities, while admitting stock may not be free. Labels FFI-105 and FFI-106 Not affected solely because nothing links them to the truck. No allocation, capacity or complete dependency data supports either assurance. Notes flag the first four orders at risk. Browser and printer answers avoid this. Evidence: p3/q3.txt and tools.json, shots/p3-q3-transcript.png, materials/ops-notes.md. Related unsupported assumption: Q1 says the real stock total is a bit higher despite missing costs, p3/q1.txt.

### F8, Low: half-cent average-cost money differs between app and MCP prose

Repro: required Southern Ice Q4. Exact scenario average unit cost is 85.335 AUD, a half-cent amount. App card and gpt-6-luna prose retain 85.335, while MCP agent prose says $85.34. The app retains three decimals so 85.180 to 85.335 adds up to +0.155, and the MCP cents value rounds half away from zero correctly. Their difference is only 0.005, within numeric tolerance, but the explicit requirement that any half-cent money amount display identically is not met. Stock total, stock delta, Block Ice and carton money checks pass. This is output consistency, not arithmetic or banker's rounding. Evidence: p3/q4.txt, p4/q4.txt, shots/p3-q4-transcript.png, shots/p4-q4-top.png, ev/display-number-check.json.

## Publication and cleanup

Screenshots, transcript images, raw fixtures, transcripts and selected receipts are published through a temporary detached worktree on pr-screenshots without force. Protected credentials, MCP config, environments, startup scripts and product source are excluded. ev/upload.json records the verified remote commit.

The isolated backend/frontend are stopped, eunomia-e2e-fresh4 is stopped and removed, and this worktree's backend/target is deleted. ev/cleanup.json checks ports 8481, 3480 and 8100, container absence, target absence, git status and other containers. Temporary publication worktree removal is recorded locally after pushing.

## Browser timings

| Question | Seconds |
|---|---:|
| What is our average sell price and total stock value at cost | 15.046866 |
| Reefer 2 is running a day late. What does that affect? | 11.768787 |
| What is happening with Esperance Abalone? | 11.531846 |
| Office Label Printer is broken. What does that affect? | 8.700299 |
| What if Southern Ice Works raises costs 10%? | 13.256474 |

Times use persisted first user-message creation to final assistant-message creation, not polling or screenshot time. The later forced one-second timeout probe is separate.

## Numeric audit appendix

These are all structured metric checks plus separately audited display, delta, relative-percentage and changed-row numbers. Tolerance 0.01. Displayed endpoints must subtract to displayed delta exactly. Import values and operational quantities/dates are separately checked against CSV/notes and recorded in import verification and DB export.

| Context / metric | Actual | Independent | Result |
|---|---:|---:|---|
| direct available_product_count | 26.0 | 26 | PASS |
| direct avg_sell_price | 130.793333 | 130.7933333333333333333333333 | PASS |
| direct avg_unit_cost | 85.180357 | 85.18035714285714285714285714 | PASS |
| direct gross_margin | 35.76142 | 35.76141995259642318465847878 | PASS |
| direct product_count | 30.0 | 30 | PASS |
| direct total_stock_value_at_cost | 27424.5 | 27424.50 | PASS |
| q1.tools avg_sell_price | 130.793333 | 130.7933333333333333333333333 | PASS |
| q1.tools total_stock_value_at_cost | 27424.5 | 27424.50 | PASS |
| q4.tools baseline gross_margin | 35.76142 | 35.76141995259642318465847878 | PASS |
| q4.tools baseline avg_unit_cost | 85.180357 | 85.18035714285714285714285714 | PASS |
| q4.tools baseline total_stock_value_at_cost | 27424.5 | 27424.50 | PASS |
| q4.tools scenario gross_margin | 35.644796 | 35.64479638009049773755656109 | PASS |
| q4.tools scenario avg_unit_cost | 85.335 | 85.335 | PASS |
| q4.tools scenario total_stock_value_at_cost | 27639.875 | 27639.875 | PASS |
| q2.tools gross_margin | 35.680333 | 35.68033273915626856803327392 | PASS |
| chat baseline avg_unit_cost | 85.180357 | 85.18035714285714285714285714 | PASS |
| chat baseline gross_margin | 35.76142 | 35.76141995259642318465847878 | PASS |
| chat baseline total_stock_value_at_cost | 27424.5 | 27424.50 | PASS |
| chat scenario avg_unit_cost | 85.335 | 85.335 | PASS |
| chat scenario gross_margin | 35.644796 | 35.64479638009049773755656109 | PASS |
| chat scenario total_stock_value_at_cost | 27639.875 | 27639.875 | PASS |
| chat total_stock_value_at_cost | 27424.5 | 27424.50 | PASS |
| chat avg_sell_price | 130.793333 | 130.7933333333333333333333333 | PASS |

| Display / changed row | Actual | Independent | Result |
|---|---:|---:|---|
| MCP Q1 average 130.79 | 130.79 | 130.7933333333333333333333333 | PASS |
| MCP Q1 cost 27424.50 | 27424.5 | 27424.50 | PASS |
| MCP Q2 margin 35.7% | 35.7 | 35.68033273915626856803327392 | FAIL |
| MCP Q4 baseline margin | 35.76 | 35.76141995259642318465847878 | PASS |
| MCP Q4 scenario margin | 35.64 | 35.64479638009049773755656109 | PASS |
| MCP Q4 margin delta | -0.12 | -0.11662357250592544710191769 | PASS |
| MCP Q4 cost before | 27424.5 | 27424.50 | PASS |
| MCP Q4 cost after | 27639.88 | 27639.875 | PASS |
| MCP Q4 cost delta | 215.38 | 215.375 | PASS |
| MCP Q4 avg cost before | 85.18 | 85.18035714285714285714285714 | PASS |
| MCP Q4 avg cost after | 85.34 | 85.335 | PASS |
| UI avg cost before | 85.180 | 85.18035714285714285714285714 | PASS |
| UI avg cost after | 85.335 | 85.335 | PASS |
| UI avg cost delta | 0.155 | 0.15464285714285714285714286 | PASS |
| UI avg cost shown end minus start | 0.155 | 0.155 | PASS |
| UI gross margin before | 35.76 | 35.76141995259642318465847878 | PASS |
| UI gross margin after | 35.64 | 35.64479638009049773755656109 | PASS |
| UI gross margin delta | -0.12 | -0.11662357250592544710191769 | PASS |
| UI gross margin shown end minus start | -0.12 | -0.12 | PASS |
| UI stock cost before | 27424.50 | 27424.50 | PASS |
| UI stock cost after | 27639.88 | 27639.875 | PASS |
| UI stock cost delta | 215.38 | 215.375 | PASS |
| UI stock cost shown end minus start | 215.38 | 215.38 | PASS |
| MCP Q4 margin shown endpoints subtract to stated delta | -0.12 | -0.12 | PASS |
| MCP Q4 money shown endpoints subtract to stated delta | 215.38 | 215.38 | PASS |
| UI avg cost relative change 0.2% | 0.2 | 0.1815475566549967505922307742 | FAIL |
| UI stock cost relative change 0.8% | 0.8 | 0.7853379277653193312548998159 | FAIL |
| p3 exact delta gross_margin | -0.116624 | -0.11662357250592544710191769 | PASS |
| p3 relative delta gross_margin | -0.33 | -0.3261156086808785755764667343 | PASS |
| p3 exact delta avg_unit_cost | 0.154643 | 0.15464285714285714285714286 | PASS |
| p3 relative delta avg_unit_cost | 0.18 | 0.1815475569594726432728414839 | PASS |
| p3 exact delta total_stock_value_at_cost | 215.375 | 215.375 | PASS |
| p3 relative delta total_stock_value_at_cost | 0.79 | 0.7853379277653193312548998159 | PASS |
| p3 changed before Flake Ice 10kg | 5.1 | 5.10 | PASS |
| p3 changed after Flake Ice 10kg | 5.61 | 5.610 | PASS |
| p3 changed before Party Ice 5kg | 3.8 | 3.80 | PASS |
| p3 changed after Party Ice 5kg | 4.18 | 4.180 | PASS |
| p3 changed before Gel Ice Pack 1kg | 2.4 | 2.40 | PASS |
| p3 changed after Gel Ice Pack 1kg | 2.64 | 2.640 | PASS |
| p3 changed before Ice Brick 500g | 1.8 | 1.80 | PASS |
| p3 changed after Ice Brick 500g | 1.98 | 1.980 | PASS |
| p3 changed before Crushed Ice 5kg | 3.2 | 3.20 | PASS |
| p3 changed after Crushed Ice 5kg | 3.52 | 3.520 | PASS |
| p3 changed before Insulated Seafood Carton | 6.75 | 6.75 | PASS |
| p3 changed after Insulated Seafood Carton | 7.425 | 7.425 | PASS |
| p3 changed before Block Ice 20kg | 7.25 | 7.25 | PASS |
| p3 changed after Block Ice 20kg | 7.975 | 7.975 | PASS |
| p3 changed before Dry Ice 2kg | 13.0 | 13.00 | PASS |
| p3 changed after Dry Ice 2kg | 14.3 | 14.300 | PASS |
| chat exact delta avg_unit_cost | 0.154643 | 0.15464285714285714285714286 | PASS |
| chat relative delta avg_unit_cost | 0.18 | 0.1815475569594726432728414839 | PASS |
| chat exact delta gross_margin | -0.116624 | -0.11662357250592544710191769 | PASS |
| chat relative delta gross_margin | -0.33 | -0.3261156086808785755764667343 | PASS |
| chat exact delta total_stock_value_at_cost | 215.375 | 215.375 | PASS |
| chat relative delta total_stock_value_at_cost | 0.79 | 0.7853379277653193312548998159 | PASS |
| chat changed before Flake Ice 10kg | 5.1 | 5.10 | PASS |
| chat changed after Flake Ice 10kg | 5.61 | 5.610 | PASS |
| chat changed before Party Ice 5kg | 3.8 | 3.80 | PASS |
| chat changed after Party Ice 5kg | 4.18 | 4.180 | PASS |
| chat changed before Gel Ice Pack 1kg | 2.4 | 2.40 | PASS |
| chat changed after Gel Ice Pack 1kg | 2.64 | 2.640 | PASS |
| chat changed before Ice Brick 500g | 1.8 | 1.80 | PASS |
| chat changed after Ice Brick 500g | 1.98 | 1.980 | PASS |
| chat changed before Crushed Ice 5kg | 3.2 | 3.20 | PASS |
| chat changed after Crushed Ice 5kg | 3.52 | 3.520 | PASS |
| chat changed before Insulated Seafood Carton | 6.75 | 6.75 | PASS |
| chat changed after Insulated Seafood Carton | 7.425 | 7.425 | PASS |
| chat changed before Block Ice 20kg | 7.25 | 7.25 | PASS |
| chat changed after Block Ice 20kg | 7.975 | 7.975 | PASS |
| chat changed before Dry Ice 2kg | 13.0 | 13.00 | PASS |
| chat changed after Dry Ice 2kg | 14.3 | 14.300 | PASS |
| UI half-away money Block Ice 20kg | 7.98 | 7.98 | PASS |
| UI half-away money Insulated Seafood Carton | 7.43 | 7.43 | PASS |
| UI/MCP half-away stock total | 27639.88 | 27639.88 | PASS |
| UI/MCP half-away stock delta | 215.38 | 215.38 | PASS |


| Half-cent average cost app versus MCP cents text, exact consistency rule | 85.335 | 85.34 | FAIL |
