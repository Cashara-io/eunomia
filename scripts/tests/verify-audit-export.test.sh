#!/usr/bin/env bash
# The offline security-log verifier (scripts/audit/verify-audit-export.py, docs/audit-chain.md):
# a good export passes; an edit, a gap, a truncated log, a wound-back head and a kept checkpoint the
# file does not reach all fail; --partial accepts a deliberate slice. No backend needed: the
# fixtures are built here with the script's own canonical form (the Rust side is pinned against the
# same bytes by backend/tests/audit_chain.rs).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 -I - "$ROOT/scripts/audit/verify-audit-export.py" <<'PY'
import copy, importlib.util, json, subprocess, sys, tempfile, os

script = sys.argv[1]
spec = importlib.util.spec_from_file_location("verifier", script)
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)

LEDGER = "org:0123456789abcdef0123456789abcdef"
GENESIS_AT = "2026-11-01T00:00:00.000000Z"
genesis = v.sha256_hex(v.compact([v.VERSION, "genesis", LEDGER, GENESIS_AT]))

# a completed access review, and its completion event (event 10)
SNAPSHOT = {"workspace": LEDGER, "taken_at": "2026-11-01T00:00:00.000000Z",
            "people": [{"user_id": "user:u2", "email": "bob@example.com", "role": "member"}]}
SNAPSHOT_HASH = v.sha256_hex(v.canonical_json(SNAPSHOT))
RESULT = {"format": "eunomia-access-review", "version": 1, "review_id": "access_review:r1", "workspace": LEDGER,
          "snapshot_hash": SNAPSHOT_HASH, "started_by": "user:u1", "started_at": "2026-11-01T00:00:00.000000Z",
          "completed_by": "user:u1", "completed_at": "2026-11-01T00:00:10.000000Z",
          "decisions": [{"user_id": "user:u2", "decision": "keep", "note": "", "reviewer_id": "user:u1",
                         "decided_at": "2026-11-01T00:00:05.000000Z"}],
          "not_reviewed": ["user:u1"]}
RESULT_HASH = v.sha256_hex(v.canonical_json(RESULT))
COMPLETE_DETAIL = "result=%s keep=1 remove=0 suspend=0 not_reviewed=1" % RESULT_HASH

def chain(n):
    events, prev = [], genesis
    for seq in range(1, n + 1):
        e = {"ledger": LEDGER, "seq": seq, "prev_hash": prev, "id": "audit_event:e%d" % seq,
             "created_at": "2026-11-01T00:00:%02d.000000Z" % seq, "user": "user:u1", "actor_kind": "user",
             "actor_id": "user:u1", "action": "vault.create", "target": "vault:v%d" % seq, "outcome": "ok",
             "trace_id": "t%d" % seq, "detail_digest": "0" * 64}
        if seq == 10:
            e.update(action="access_review.complete", target="access_review:r1", detail_digest=v.sha256_hex(COMPLETE_DETAIL))
        e["hash"] = v.sha256_hex(v.canonical(e))
        prev = e["hash"]
        events.append(e)
    return events

ALL = chain(10)

def export(events, anchor, to="", truncated=False, head=None, unsealed=0):
    events, head = copy.deepcopy(events), copy.deepcopy(head)
    if head is None:
        head = {"seq": events[-1]["seq"], "hash": events[-1]["hash"], "at": "", "kind": "head"} if events else None
    return {"format": "eunomia-audit-export", "version": v.VERSION, "ledger": LEDGER, "from": "", "to": to,
            "anchor": anchor if events else None, "events": events, "checkpoints": [], "head": head,
            "range": {"first_seq": events[0]["seq"] if events else None, "last_seq": events[-1]["seq"] if events else None,
                      "head_seq": head["seq"] if head else None, "head_hash": head["hash"] if head else None,
                      "unsealed": unsealed},
            "truncated": truncated, "ok": True}

GENESIS = {"seq": 0, "hash": genesis, "at": GENESIS_AT, "kind": "genesis"}
tmp = tempfile.mkdtemp(prefix="audit-verify-")
failures = []

def run(name, doc, args, want_code, want_text, review=None):
    path = os.path.join(tmp, "export.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f)
    if review is not None:
        rpath = os.path.join(tmp, "review.json")
        with open(rpath, "w", encoding="utf-8") as f:
            json.dump(review, f)
        args = args + ["--review", rpath]
    out = subprocess.run([sys.executable, "-I", script, path] + args, capture_output=True, text=True)
    ok = out.returncode == want_code and want_text in out.stdout
    print(("pass" if ok else "FAIL") + ": " + name + ("" if ok else " (exit %d) %s" % (out.returncode, out.stdout.strip())))
    if not ok:
        failures.append(name)

kept10 = "10:" + ALL[9]["hash"]
kept2 = "2:" + ALL[1]["hash"]

run("a whole export verifies", export(ALL, GENESIS), [], 0, "Events 1 to 10")
run("a kept checkpoint inside the file matches", export(ALL, GENESIS), ["--checkpoint", kept10], 0, "verify")

edited = export(ALL, GENESIS)
edited["events"][4]["outcome"] = "auth.forbidden"
run("an edited event fails", edited, [], 1, "Event 5 was changed")

gap = export(ALL[:3] + ALL[4:], GENESIS)
run("a deleted middle event fails", gap, [], 1, "Event 4 is missing")

# the attack: newest events deleted and the head wound back to a stored hash; the file is consistent
cut = export(ALL[:7], GENESIS)
run("a truncated log alone looks whole", cut, [], 0, "Events 1 to 7")
run("a truncated log fails against the checkpoint the auditor kept", cut, ["--checkpoint", kept10], 1,
    "newer events are missing, or the log was wound back")

rewound = export(ALL[:7], GENESIS, head={"seq": 10, "hash": ALL[9]["hash"], "at": "", "kind": "head"})
run("an export that stops short of its own head fails", rewound, [], 1, "ends at event 7 but its own head is event 10")

# a prefix deleted with no retention checkpoint: the server can only anchor at the first event's prev_hash
prefix = export(ALL[3:], {"seq": 3, "hash": ALL[2]["hash"], "at": "", "kind": "export"})
run("a log missing its oldest events fails against an older kept checkpoint", prefix, ["--checkpoint", kept2], 1,
    "The export starts at event 4 but you hold a checkpoint for event 2")

HEAD10 = {"seq": 10, "hash": ALL[9]["hash"], "at": "", "kind": "head"}
slice_ = export(ALL[3:7], {"seq": 3, "hash": ALL[2]["hash"], "at": "", "kind": "export"}, to="2026-11-01T00:00:08.000000Z", head=HEAD10)
run("a date-range slice verifies on its own", slice_, [], 0, "Events 4 to 7")
run("a slice fails a kept checkpoint past its end without --partial", slice_, ["--checkpoint", "9:" + ALL[8]["hash"]], 1, "rerun with --partial")
run("--partial accepts a kept checkpoint between the slice's end and its head", slice_, ["--checkpoint", "9:" + ALL[8]["hash"], "--partial"], 0, "not checked: --partial")
run("a kept checkpoint at the recorded head is checked even with --partial", slice_, ["--checkpoint", "10:" + "f" * 64, "--partial"], 1, "does not match the head")
run("a kept checkpoint at the recorded head that matches passes", slice_, ["--checkpoint", kept10], 0, "Events 4 to 7")

# the reviewer's case 2: retention anchor at 3, events 4-7, head wound back to 7, kept 2 and 10, --partial
cut_back = export(ALL[3:7], {"seq": 3, "hash": ALL[2]["hash"], "at": "", "kind": "retention"})
run("a wound-back head fails a newer kept checkpoint even with --partial", cut_back, ["--checkpoint", kept2, "--checkpoint", kept10, "--partial"], 1,
    "head was event 7 when the file was made, but you hold a checkpoint for event 10")
run("a retention cut verifies an older kept checkpoint without --partial", export(ALL[3:], {"seq": 3, "hash": ALL[2]["hash"], "at": "", "kind": "retention"}),
    ["--checkpoint", kept2, "--checkpoint", kept10], 0, "older than the retention cut")

edited_header = export(ALL, GENESIS)
edited_header["range"]["last_seq"] = 12
run("an edited header fails", edited_header, [], 1, "header says events")
moved_head = export(ALL[:7], GENESIS, head=HEAD10)
moved_head["head"]["seq"] = 7
run("a head edited to match a cut file still disagrees with its header", moved_head, [], 1, "header and its head disagree")

# the reviewer's case 1: the head row deleted, so the server exported nothing
empty = export([], None, head=None)
run("an empty export fails against a kept checkpoint", empty, ["--checkpoint", kept10], 1, "the log was emptied")
run("an empty export fails by default", empty, [], 1, "--expect-empty")
run("--expect-empty accepts an empty log", empty, ["--expect-empty"], 0, "as expected")
run("--expect-empty still fails a kept checkpoint", empty, ["--checkpoint", kept10, "--expect-empty"], 1, "the log was emptied")
quiet_day = export([], None, to="2026-11-02T00:00:00.000000Z", head=HEAD10)
quiet_day["from"] = "2026-11-01T12:00:00.000000Z"
run("a date range with no events passes", quiet_day, ["--checkpoint", kept10], 0, "No events in the requested range")
run("a date range with no events still fails a kept checkpoint above its head", quiet_day, ["--checkpoint", "11:" + "a" * 64], 1, "you hold a checkpoint for event 11")
broken = export(ALL, GENESIS)
broken["ok"], broken["break"] = False, {"seq": None, "kind": "head", "message": "This log has events or checkpoints but no head."}
run("a file the server marked broken fails", broken, [], 1, "no head")
run("the unsealed count is reported", export(ALL, GENESIS, unsealed=3), [], 0, "3 event(s) were not yet sealed")

# an access review against the chain
REVIEW = {"result": RESULT, "result_hash": RESULT_HASH, "snapshot": SNAPSHOT, "snapshot_hash": SNAPSHOT_HASH,
          "audit_event": {"ledger": LEDGER, "seq": 10, "hash": ALL[9]["hash"], "detail": COMPLETE_DETAIL}}
run("a completed review ties to its completion event", export(ALL, GENESIS), [], 0, "is the one the log recorded at event 10", review=REVIEW)
rewritten = copy.deepcopy(REVIEW)
rewritten["result"]["decisions"][0]["decision"] = "remove"
rewritten["result_hash"] = v.sha256_hex(v.canonical_json(rewritten["result"]))
run("a review rewritten after completion (hash recomputed) fails against the chain", export(ALL, GENESIS), [], 1,
    "records another result", review=rewritten)
lied = copy.deepcopy(rewritten)
lied["audit_event"]["detail"] = "result=%s keep=0 remove=1 suspend=0 not_reviewed=1" % rewritten["result_hash"]
run("a forged completion detail fails", export(ALL, GENESIS), [], 1, "does not commit to the detail", review=lied)
snap = copy.deepcopy(REVIEW)
snap["snapshot"]["people"][0]["role"] = "admin"
run("an edited snapshot fails", export(ALL, GENESIS), [], 1, "snapshot was changed", review=snap)

forged = export(ALL, dict(GENESIS, at="2026-11-02T00:00:00.000000Z"))
run("a changed genesis fails", forged, [], 1, "starting point was changed")

if failures:
    sys.exit("verify-audit-export: %d case(s) failed" % len(failures))
print("verify-audit-export: all cases passed")
PY
