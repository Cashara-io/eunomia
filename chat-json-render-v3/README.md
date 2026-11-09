# Chat UI review proof

Source head: `80355d669b51664094da67ac09c4a317228f6b4a` on `feat/chat-json-render`.
Base content retained: `e5b725097f2b6ef820726eda680b0ffcae3e926f`.

Six fresh in-app answers used `gpt-6-luna` on isolated ports 8590, 8591 and 3590. The twelve-row CSV was uploaded, approved and imported into a disposable account. All six final answer screenshots were opened and inspected. The numeric audit matched 379 fields within 0.01. The four catalogue contact sheets cover light and dark at 1280 and 390 px and were inspected too.

`mutations/mutations.json` records fourteen detected backend guard removals. The frontend missing-measurement guard removal is in `checks/chat-v3-frontend-mutation.log.test`. All guards were restored. A compiler-error attempt is retained separately and is not counted as a detected mutation.

`checks/chat-v3-backend-final.log` contains 514 library tests and the hardened integration run. The scale fixture confirms 5000 entities, 50000 history rows, grouping in 1164 ms and monthly bucketing in 2152 ms, both using the named indexes. Per-pair timestamp tie sorting remains visible in EXPLAIN. The initial scale run is retained too.

Browser checks passed with model access disabled. A backend-restart race is retained in the logs; every-tool passed after the readiness check. Frontend lint, typecheck and build passed. Clippy on all targets passed with warnings denied.

The full command list and test-to-guard table are in the source repository's `docs/proofs/chat-json-render.md` and `chat-json-render-review-commands.txt`.
