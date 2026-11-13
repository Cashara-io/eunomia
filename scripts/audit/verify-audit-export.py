#!/usr/bin/env python3
"""Verify a Eunomia security log export offline (docs/audit-chain.md).

    python3 scripts/audit/verify-audit-export.py EXPORT.json [--checkpoint SEQ:HASH ...]

Recomputes every event's SHA-256 from its canonical form, checks that each event follows the one
before it (sequence and previous hash) from the file's anchor, and that every checkpoint in the file,
and every --checkpoint you kept elsewhere (for example a periodic checkpoint copied off the server),
matches the event at its sequence number. Exits 0 when everything verifies, 1 at the first break
(printed), 2 when the file cannot be read. Needs only the Python standard library.
"""
import hashlib
import json
import sys

VERSION = "eunomia.audit.v1"
FIELDS = ["ledger", "seq", "prev_hash", "id", "created_at", "user", "actor_kind", "actor_id",
          "action", "target", "outcome", "trace_id", "detail_sha256"]


def sha256_hex(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def canonical(event):
    return compact([VERSION] + [event[f] for f in FIELDS])


def verify(doc, kept):
    if doc.get("format") != "eunomia-audit-export" or doc.get("version") != VERSION:
        return False, "This is not a %s security log export." % VERSION
    ledger = doc["ledger"]
    events = doc.get("events") or []
    if not events:
        return True, "No events in this file."
    anchor = doc.get("anchor")
    if not anchor:
        return False, "The file has no anchor to start from."
    if anchor.get("kind") == "genesis" and anchor["seq"] == 0:
        if sha256_hex(compact([VERSION, "genesis", ledger, anchor["at"]])) != anchor["hash"]:
            return False, "The chain's starting point was changed."
    expect, prev = anchor["seq"] + 1, anchor["hash"]
    hashes = {anchor["seq"]: anchor["hash"]}
    for e in events:
        missing = [f for f in FIELDS + ["hash"] if f not in e]
        if missing:
            return False, "An event lacks %s." % ", ".join(missing)
        seq = e["seq"]
        if e["ledger"] != ledger:
            return False, "Event %d belongs to another ledger." % seq
        if seq < expect:
            return False, "Event %d appears more than once." % seq
        if seq > expect:
            return False, ("Event %d is missing." % expect) if seq == expect + 1 else ("Events %d to %d are missing." % (expect, seq - 1))
        if e["prev_hash"] != prev:
            return False, "Event %d does not follow the event before it." % seq
        if sha256_hex(canonical(e)) != e["hash"]:
            return False, "Event %d was changed after it was written." % seq
        hashes[seq] = e["hash"]
        prev, expect = e["hash"], seq + 1
    for cp in list(doc.get("checkpoints") or []) + kept:
        if cp["seq"] in hashes and hashes[cp["seq"]] != cp["hash"]:
            return False, "The checkpoint at event %d does not match the event." % cp["seq"]
    return True, "Events %d to %d of %s verify (%d events)." % (events[0]["seq"], events[-1]["seq"], ledger, len(events))


def main(argv):
    if len(argv) < 2 or argv[1] in ("-h", "--help"):
        print(__doc__.strip())
        return 2
    kept = []
    args = argv[2:]
    while args:
        if args[0] != "--checkpoint" or len(args) < 2 or ":" not in args[1]:
            print("Unknown argument: %s" % args[0], file=sys.stderr)
            return 2
        seq, digest = args[1].split(":", 1)
        kept.append({"seq": int(seq), "hash": digest.strip().lower()})
        args = args[2:]
    try:
        with open(argv[1], encoding="utf-8") as f:
            doc = json.load(f)
    except (OSError, ValueError) as e:
        print("Cannot read %s: %s" % (argv[1], e), file=sys.stderr)
        return 2
    try:
        ok, message = verify(doc, kept)
    except (KeyError, TypeError) as e:
        ok, message = False, "The file is not a complete export (%s)." % e
    print(("OK: " if ok else "BROKEN: ") + message)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
