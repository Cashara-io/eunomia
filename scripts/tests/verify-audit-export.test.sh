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

def chain(n):
    events, prev = [], genesis
    for seq in range(1, n + 1):
        e = {"ledger": LEDGER, "seq": seq, "prev_hash": prev, "id": "audit_event:e%d" % seq,
             "created_at": "2026-11-01T00:00:%02d.000000Z" % seq, "user": "user:u1", "actor_kind": "user",
             "actor_id": "user:u1", "action": "vault.create", "target": "vault:v%d" % seq, "outcome": "ok",
             "trace_id": "t%d" % seq, "detail_digest": "0" * 64}
        e["hash"] = v.sha256_hex(v.canonical(e))
        prev = e["hash"]
        events.append(e)
    return events

ALL = chain(10)

def export(events, anchor, to="", truncated=False, head=None):
    last = events[-1]
    events = copy.deepcopy(events)
    return {"format": "eunomia-audit-export", "version": v.VERSION, "ledger": LEDGER, "from": "", "to": to,
            "anchor": anchor, "events": events, "checkpoints": [],
            "head": head or {"seq": last["seq"], "hash": last["hash"], "at": "", "kind": "head"},
            "truncated": truncated, "ok": True}

GENESIS = {"seq": 0, "hash": genesis, "at": GENESIS_AT, "kind": "genesis"}
tmp = tempfile.mkdtemp(prefix="audit-verify-")
failures = []

def run(name, doc, args, want_code, want_text):
    path = os.path.join(tmp, "export.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(doc, f)
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
    "The export ends at event 7 but you hold a checkpoint for event 10")

rewound = export(ALL[:7], GENESIS, head={"seq": 10, "hash": ALL[9]["hash"], "at": "", "kind": "head"})
run("an export that stops short of its own head fails", rewound, [], 1, "ends at event 7 but its own head is event 10")

prefix = export(ALL[3:], {"seq": 3, "hash": ALL[2]["hash"], "at": "", "kind": "retention"})
run("a log missing its oldest events fails against an older kept checkpoint", prefix, ["--checkpoint", kept2], 1,
    "The export starts at event 4 but you hold a checkpoint for event 2")

slice_ = export(ALL[3:7], {"seq": 3, "hash": ALL[2]["hash"], "at": "", "kind": "export"}, to="2026-11-01T00:00:08.000000Z",
                head={"seq": 10, "hash": ALL[9]["hash"], "at": "", "kind": "head"})
run("a date-range slice verifies on its own", slice_, [], 0, "Events 4 to 7")
run("a slice fails a kept checkpoint it does not reach", slice_, ["--checkpoint", kept10], 1, "rerun with --partial")
run("--partial accepts a deliberate slice", slice_, ["--checkpoint", kept10, "--checkpoint", kept2, "--partial"], 0, "not checked: --partial")
run("--partial still fails a kept checkpoint that disagrees",
    export(ALL, GENESIS), ["--checkpoint", "10:" + "f" * 64, "--partial"], 1, "does not match")

forged = export(ALL, dict(GENESIS, at="2026-11-02T00:00:00.000000Z"))
run("a changed genesis fails", forged, [], 1, "starting point was changed")

if failures:
    sys.exit("verify-audit-export: %d case(s) failed" % len(failures))
print("verify-audit-export: all cases passed")
PY
