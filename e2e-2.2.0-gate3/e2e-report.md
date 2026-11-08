# Eunomia 2.2.0 release gate 3

**Verdict: FAIL.** The fresh business flow works, and all 25 structured numeric checks pass. Two gate 2 gaps remain in narrower forms: N2 still selects Personal when the phrase extractor returns an empty list, and N9 still leaks snake_case in scenario warnings. A new Medium issue makes the scenario card's margin change inaccurate beyond the required 0.01 tolerance. No High defect was reproduced in this run. No product code was changed.

- Main SHA: `ef8f0023b6b04ca2524bd68bb504823a6af997cc`, detached from `origin/main`. Includes PRs 183, 184 and 185.
- Date: 9 November 2026, Australia/Melbourne. Database timestamps are UTC on 8 November.
- Backend package still identifies itself as 2.1.0. This is a release-version observation, not proof of a runtime failure.
- Isolated stack: `surrealdb/surrealdb:v3.3.5`, fresh in-memory Docker database named `eunomia-e2e-fresh3`, host 127.0.0.1:8100. Compose security flags retained. A fresh host documents folder is mounted at `/data/documents`; backend uses `file:/data/documents`.
- Backend 127.0.0.1:8481, frontend 127.0.0.1:3480. Real in-app model `gpt-6-luna`, unchanged. No mocks.
- Backend built only with the supplied shared cargo-lock script, in 2m 58s. No cargo fmt. `bun install` succeeded. Disk check started at 18 GiB free, above the 12 GB minimum.
- Fresh first admin registered and onboarded through the same endpoints as `scripts/ci/e2e-setup.sh`. Credentials and MCP config are mode 600 and excluded from uploads.
- Business: Top End Solar & Electrical, Darwin NT. Unrelated Darwin Footy Club has three memories. The admin owns the default Personal. All three memberships verified in SQL.
- Raw `materials/stock.csv`: 30 items, four suppliers, Solar Juice case variants, two blank costs, one zero quantity, Australian money and day-first dates. `materials/jobs-notes.md`: six customers and jobs, delivery IN-77 on Ute 3, a dated fee decision, an unconfirmed price rumour and an unlinked printer. No pre-mapping.
- Evidence paths below are relative to `/private/tmp/eunomia-gate3/`. Screenshots and transcript images are also published under `e2e-2.2.0-gate3/` on `pr-screenshots`.

## Step results

| Step | Result | Evidence |
|---|---|---|
| Fresh main checkout, disk threshold, locked backend build, bun install | PASS | `ev/main-sha.txt`, `ev/build.log`, `ev/bun-install.log` |
| Fresh database, documents folder, admin and token | PASS | `ev/container.txt`, `ev/counts-start.json`, `shots/p0-login.png` |
| Mapping agent uses only Eunomia business tools | PASS | `p1/transcript.jsonl`, `p1/transcript.tools.json`; built-in ToolSearch only discovers the MCP tools |
| Creates org and uploads both raw files | PASS | `p1/transcript.tools.json`, `shots/p2-documents.png` |
| Stores jobs, customers, suppliers, decision and rumour with sensible kinds | PASS | `p1/transcript.txt`, `ev/independent-db.json` |
| Makes actual business links with entity_relate | PASS | 42 successful calls, 42 relations before import, `ev/counts-after-p1.json` |
| Proposes typed ontology and certified metrics, approves nothing | PASS | 9 proposed kinds, 12 properties, 8 metrics, 11 link types; `ev/ontology-before.json` |
| Browser owner reviews Kinds and links | PASS | `shots/p2-kinds-before.png`, `shots/p2-kinds-approved.png`, `ev/owner-decisions.json` |
| Browser approvals | PASS | 43 HTTP 200 decisions. One supplier merge rejection was later superseded by approval, so the final approved ontology keeps the unused supplier kind and both supplied-by link types. This is disclosed test behaviour, not a retained merge claim. |
| CSV import | PASS | 30 rows, 13 created, 17 updated, 178 typed values, 30 links, zero targets created, zero bad values; `ev/import-response.json` |
| Existing suppliers reused and spelling variants unified | PASS | Four organisation IDs reused, no supplier/customer duplicates; `ev/import-verify.json` |
| CSV mapping label layout | PASS | Ellipsis and complete title tooltip, no spill into Type; `shots/p2-csv-approved.png` |
| Readiness and strict switch | PASS | Ready: 11 kinds, 12 properties, 8 metrics, 11 links; `shots/p2-readiness-final.png`, `shots/p2-strict-on.png` |
| MCP Q1, average price and stock at cost, no space named | PASS | $505.84 and $67,775.75; `p3/q1.tools.json`, `shots/p3-q1-transcript.png` |
| MCP Q2, supplier margin | PASS | related_to used; 35.275267%, seven costed Fronius items, blank-cost row excluded from both sides; `p3/q2.tools.json` |
| MCP Q3, Ute delay | PASS | neighbourhood and impact show_view follow delivery and job links; `p3/q3.tools.json` |
| MCP Q4, Rexel costs +10% | PARTIAL | Correct scenario_run, scenario show_view, no writes. Coverage spuriously calls raises an uncovered term. Prose gives rounded margin change outside tolerance; `p3/q4.tools.json`, `p3/q4.txt` |
| MCP Q5, insurance | PASS | Low coverage, searches facts and documents, refuses to invent a number; `p3/q5.txt` |
| MCP Q6, strict write without a space named | PASS | Agent chooses Top End, proposal filed, memory_write refused; `p3/q6.tools.json` |
| Direct no-vault strict write guard | PASS | Refuses and names all three spaces; `ev/strict-no-vault-direct.json` |
| Same strict write with org named | PASS | Refused, ev_charger remains proposed with zero entities; `p3/q6-named.tools.json`, `shots/p3-strict-proposal.png` |
| In-app Q1 without picking a space | PASS | Correct metric cards, 13.957 seconds; `shots/p4-q1-chat-expanded.png` |
| In-app Q3 without picking a space | PASS | Correct delivery and four job risks, 14.214 seconds; `shots/p4-q3-impact-full.png` |
| In-app Q4 without picking a space | PARTIAL | Correct scope, scenario, percent units, linked-within wording and stock delta, 15.649 seconds. Percent rounding fails 0.01 tolerance; `shots/p4-q4-scenario-full.png` |
| Unlinked printer impact | PASS | Says no saved links, does not claim nothing is affected; 7.596 seconds; `shots/p4-unlinked-chat.png` |
| All required chat answers complete, none hangs | PASS | `ev/chat-timings-verified.json`, `ev/chat-final.json` |
| Independent calculations | PARTIAL | 25/25 structured numeric checks pass; visible percent endpoints and delta fail the stated tolerance; `ev/numbers-check.json`, `ev/display-number-check.json` |
| Both scenarios save nothing | PASS | entity 52, entity_property 178, relates_to 72, memory 77 before and after both runs; counts files |
| Five random/required import rows, plus all 30 rows | PASS | Zero mismatches. Includes $4,890.00, 07/11/2026, both blanks and zero stock; `ev/import-verify.json` |
| Duplicate and space-isolation checks | PASS | No repeated supplier/customer names across built-in and custom kinds. Personal empty, Footy only its three topics; `ev/import-verify.json` |
| Transcript images and screenshots | PASS | All eight headless transcripts have JSONL, text, expandable review HTML and a PNG. Full chat captures expand only scroll containers; `ev/screenshot-capture-note.txt` |
| Upload and server teardown | PASS | `ev/cleanup.json`, `ev/upload.json` |

## Gate 2 defects

| Defect | Status | Evidence and limits |
|---|---|---|
| N1, no-space write bypasses strict mode | FIXED | No entity or memory added. Direct no-vault write refuses and names Personal, Darwin Footy Club and Top End. Both agent variants refuse in Top End and leave a proposal. `ev/strict-no-vault-direct.json`, `p3/q6.tools.json`, `p3/q6-named.tools.json` |
| N2, unmatched question selects Personal | NOT FIXED, partial | Required costs scenario and numeric unknown-term fallback use Top End correctly. But `What is happening with Label Printer 2?` twice, and `Tell me about Ute 3`, extract zero phrases and return Personal, chosen_by coverage. Both entities exist only in Top End. `ev/entity-only-coverage.json`, `ev/scope-probe-0.json`, `ev/scope-probe-1.json`. The entity fallback is guarded by non-empty phrases in `backend/src/ontology/coverage.rs:450`. Current user-facing impact answers still work through entity search. Residual severity Low. |
| N3, metric cannot filter through supplier link | FIXED | Q2 metric_eval uses related_to supplied_by Fronius Australia; eight linked products, seven eligible; `p3/q2.tools.json` |
| N4, ratio uses different rows on each side | FIXED | Exact approved unweighted ratio recomputes to 35.756120% overall and 35.275267% for Fronius. Warning explains shared eligibility. Scenario recomputes to 35.435683%. `ev/numbers-check.json` |
| N5, fraction labelled percent | FIXED | API returns scaled values with unit %, card shows 35.8% rather than 0.36 percent. Separate rounding defect below. `shots/p4-q4-scenario-full.png` |
| N6, duplicate same-name suppliers/customers | FIXED | All four organisations reused, zero targets created, all 30 supplied_by links verified, Solar Juice variants share one organisation, no duplicate customer names. `ev/import-response.json`, `ev/import-verify.json` |
| N7, empty impact says nothing else is affected | FIXED | Printer card explicitly says no saved links and nothing to follow. `shots/p4-unlinked-chat.png` |
| N8, no business-link tool | FIXED | Mapping agent made 42 entity_relate calls. Delivery/job/stock links produce real impact paths. `p1/transcript.tools.json`, `shots/p4-q3-impact-full.png` |
| N9, raw identifiers and snake_case in UI | NOT FIXED, partial | Readiness and auto-map reasons are readable. Scenario warning still starts gross_margin; strict proposal reason contains ev_charger. MCP Q1 prose also prints two entity IDs, an agent output quality issue. `shots/p4-q4-scenario-full.png`, `shots/p3-strict-proposal.png`, `p3/q1.txt` |
| N10, CSV label spills into Type | FIXED | Label is deliberately abbreviated with an ellipsis, with full title tooltip. It stays inside its own column. `shots/p2-csv-approved.png`, `ev/csv-label-layout.json` |
| N11, costs does not match unit_cost | FIXED | Both exact scenario questions match costs to approved product unit_cost at distance 0.0. MCP additionally extracts raises, a separate false-negative diagnostic below. `p3/q4.tools.json`, `ev/chat-final.json` |

## Numbers and independent recomputation

The approved gross_margin expression is `sum(sell_price - unit_cost) / sum(sell_price)`, unit percent. It is a ratio of unit-price sums, not quantity weighted. Only rows with both prices and costs participate on both sides. Python uses Decimal, reads the raw CSV and the approved expression, and runs with `python3 -I`. Every structured check uses absolute tolerance 0.01.

| Quantity | Server or agent | Independent value | Result |
|---|---:|---:|---|
| Average sell price, 30 items | 505.838333 | 505.838333333 | PASS |
| Average unit cost, 28 costed items | 336.712500 | 336.712500 | PASS |
| Total stock at cost | 67,775.75 | 67,775.75 | PASS |
| Total stock at sell price, all rows | 114,123.50 | 114,123.50 | PASS |
| Gross margin, 28 costed items | 35.756120% | 35.756119997956% | PASS |
| Fronius margin, seven costed items | 35.275267% | 35.275267050123% | PASS |
| Units on hand | 1,270 | 1,270 | PASS |
| Back-ordered items | 3 | 3 | PASS |
| Jobs | 6 | 6 | PASS |
| Rexel eligible cost changes | 7 | 7, of 8 supplier products | PASS |
| Scenario average cost | 338.391964 | 338.391964285714 | PASS |
| Scenario average cost change | 1.679464 | 1.679464285714 | PASS |
| Scenario stock value at cost | 68,606.625 | 68,606.625 | PASS |
| Scenario stock cost increase | 830.875 | 830.875 | PASS |
| Scenario gross margin | 35.435683% | 35.435682526703% | PASS |
| Exact margin change | -0.320437 points | -0.320437471253 points | PASS |
| UI margin before | 35.8% | 35.756119997956% | FAIL, error 0.043880 points |
| UI margin after | 35.4% | 35.435682526703% | FAIL, error 0.035683 points |
| UI and MCP prose margin drop | 0.4 points | 0.320437471253 points | FAIL, error 0.079563 points |

For clarity, a quantity-weighted margin would be 38.598633% overall and 36.248958% for Fronius, but neither is the approved metric here. No answer was marked wrong for using the approved unweighted expression.

The seven exact Rexel cost deltas are 24.50, 2.80, 3.80, 2.40, 1.80, 11.00 and 0.725 AUD. Their unweighted sum is 47.025; divided by 28 costed rows it gives the 1.679464 average-cost change. Multiplying each by its stock quantity and summing gives 830.875 AUD. Rounded card endpoints and displayed deltas subtract consistently, but the percent loss is outside tolerance. The half-cent stock total may display as $68,606.63 in the UI or $68,606.62 in MCP prose; either differs from the exact total by only $0.005.

All 25 structured checks, including every approved metric and both scenario result sets, are listed individually in `ev/numbers-check.json`. Display precision failures are separate in `ev/display-number-check.json`.

## New defects

### G1, Medium: rounding percentage endpoints first overstates the margin drop

Repro: import and approve this fixture, then in Chat with no space chosen ask `What if Rexel Darwin raises costs 10%?`. The exact API baseline is 35.756120%, scenario 35.435683%, delta -0.320437 points. The card displays 35.8 to 35.4 and says down 0.4 points. This overstates the exact decline by 0.079563 points, outside 0.01. The MCP Q4 final answer repeats the rounded 0.4-point claim.

The card subtracts formatted endpoint values in `frontend/src/components/chat/views/ScenarioView.tsx:24`. Its delta does add up to its own displayed endpoints, but it is inaccurate against the approved calculation. There is no underlying metric-evaluation error.

Evidence: `shots/p4-q4-scenario-full.png`, `p4/q4.txt`, `p3/q4.txt`, `p3/q4.tools.json`, `ev/display-number-check.json`.

### G2, Low: coverage invents a missing raises term for a supported scenario

Repro: in the fresh MCP Q4 session ask the exact required costs question. ontology_coverage maps costs to unit_cost, but also extracts raises, reports coverage 0.5 and recommends proposing missing ontology for raises. There is already enough approved data and functionality to run this scenario. Claude proceeds and gets the right numbers, while the in-app run of the same question gets coverage 1.0. This is a false-negative diagnostic with model-dependent reproduction.

Evidence: `p3/q4.tools.json` contains the exact coverage response and successful scenario call; `p3/q4.txt` repeats that the setup lacks a rule for supplier raises costs; `ev/chat-final.json` records the in-app coverage 1.0.

## Residual defects and observations

N2's residual empty-phrase routing and N9's remaining warning labels are documented above with exact repros and evidence. They are Low severity in this run. They still fail the requested complete closure of gate 2.

The headless Q3 answer calls TES-103 tight but fine, based on dates alone, despite no saved job duration or scheduling capacity. Treat that as an unsupported model forecast, not proof that the job can finish. The in-app answer more cautiously says the four jobs are at risk. Evidence: `p3/q3.txt` and `p4/q3.txt`.

The owner UI was not modified. Full chat evidence expands only internal scroll containers so the whole card fits in its screenshot. Original viewport captures are retained. No production data, demo database, other containers, model settings, or product source files were changed.

## Publication and cleanup

The final receipts are `ev/upload.json` and `ev/cleanup.json`. They name the uploaded commit, target branch, published directory, removed temporary detached worktree, stopped processes, removed test container, freed ports and deleted backend/target. Secrets are retained only in protected local scratch files and are excluded from the branch.

Published evidence commit: `ea1bbf583c766155182992167f5a06762eb8bd45`, verified against the remote `pr-screenshots` head. The publication worktree is removed after the final receipt push, with that removal recorded in the local cleanup receipt.
