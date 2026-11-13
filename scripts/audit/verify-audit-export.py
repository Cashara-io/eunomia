#!/usr/bin/env python3
"""Verify a Eunomia security log export offline (docs/audit-chain.md).

    python3 scripts/audit/verify-audit-export.py EXPORT.json [--checkpoint SEQ:HASH ...] [--partial] [--expect-empty]
    python3 scripts/audit/verify-audit-export.py EXPORT.json --review REVIEW.json [--checkpoint ...]

Recomputes every event's SHA-256 from its canonical form and checks that each event follows the one
before it (sequence and previous hash) from the file's anchor. The file's header (`range`: first and
last event, the ledger's head at export time and how many events were still unsealed) must agree
with its events and with the head it records.

Checkpoints you kept elsewhere (`--checkpoint SEQ:HASH`, for example a periodic checkpoint copied off
the server) are compared with the file:
  - inside the file: it must match the event;
  - at the head the file records: it must match that head;
  - above that head: always a failure (newer events were deleted, or the head wound back);
  - between the file's last event and its head: a failure unless --partial, which says the file
    deliberately covers a sub-range (a date range or a continuation file);
  - before the file's first event: fine when the file starts at a retention checkpoint at or after
    it (the retention window deleted those events), otherwise a failure unless --partial.

A whole-log file with no events fails when you pass any checkpoint, and otherwise unless
--expect-empty says an empty log is what you expect (a date range with nothing in it passes, its
head still checked). A file the server itself marked broken fails.

--review REVIEW.json also checks a completed access review export: its snapshot and result hash
again, and that the chained completion event in EXPORT.json commits to that result hash.

Exits 0 when everything verifies, 1 at the first break (printed), 2 when a file cannot be read.
Needs only the Python standard library.
"""
import hashlib
import json
import sys

VERSION = "eunomia.audit.v1"
FIELDS = ["ledger", "seq", "prev_hash", "id", "created_at", "user", "actor_kind", "actor_id",
          "action", "target", "outcome", "trace_id", "detail_digest"]


def sha256_hex(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def canonical_json(value):
    """Keys sorted at every level, no spaces: the form access-review hashes use."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)


def canonical(event):
    return compact([VERSION] + [event[f] for f in FIELDS])


def verify(doc, kept, partial=False, expect_empty=False):
    """Returns (ok, message, events by seq)."""
    if doc.get("format") != "eunomia-audit-export" or doc.get("version") != VERSION:
        return False, "This is not a %s security log export." % VERSION, {}
    if doc.get("break") or doc.get("ok") is False:
        why = (doc.get("break") or {}).get("message") or "no reason given"
        return False, "The server reported this log as broken when it exported it: %s" % why, {}
    ledger = doc["ledger"]
    events = doc.get("events") or []
    head = doc.get("head")
    rng = doc.get("range") or {}
    unsealed = rng.get("unsealed") or 0
    notes = []
    if unsealed:
        notes.append("%d event(s) were not yet sealed when the file was made and are not in it." % unsealed)
    if not events and (doc.get("from") or doc.get("to")):
        # a date range with nothing in it: the head is still checked against what you kept
        for cp in kept:
            if not head or cp["seq"] > head["seq"]:
                return False, "The log's head was event %s when the file was made, but you hold a checkpoint for event %d." % (head and head["seq"], cp["seq"]), {}
            if cp["seq"] == head["seq"] and cp["hash"] != head["hash"]:
                return False, "Your checkpoint for event %d does not match the head the file records." % cp["seq"], {}
        return True, " ".join(["No events in the requested range."] + notes), {}
    if not events:
        if kept:
            return False, "The file has no events but you hold a checkpoint for event %d: the log was emptied." % kept[0]["seq"], {}
        if head and head.get("seq"):
            return False, "The file has no events but records a head at event %d." % head["seq"], {}
        if not expect_empty:
            return False, "The file has no events. If an empty log is what you expect, rerun with --expect-empty.", {}
        return True, " ".join(["No events in this file, as expected."] + notes), {}
    anchor = doc.get("anchor")
    if not anchor:
        return False, "The file has no anchor to start from.", {}
    if anchor.get("kind") == "genesis" and anchor["seq"] == 0:
        if sha256_hex(compact([VERSION, "genesis", ledger, anchor["at"]])) != anchor["hash"]:
            return False, "The chain's starting point was changed.", {}
    expect, prev = anchor["seq"] + 1, anchor["hash"]
    hashes = {anchor["seq"]: anchor["hash"]}
    by_seq = {}
    for e in events:
        missing = [f for f in FIELDS + ["hash"] if f not in e]
        if missing:
            return False, "An event lacks %s." % ", ".join(missing), {}
        seq = e["seq"]
        if e["ledger"] != ledger:
            return False, "Event %d belongs to another ledger." % seq, {}
        if seq < expect:
            return False, "Event %d appears more than once." % seq, {}
        if seq > expect:
            return False, (("Event %d is missing." % expect) if seq == expect + 1 else ("Events %d to %d are missing." % (expect, seq - 1))), {}
        if e["prev_hash"] != prev:
            return False, "Event %d does not follow the event before it." % seq, {}
        if sha256_hex(canonical(e)) != e["hash"]:
            return False, "Event %d was changed after it was written." % seq, {}
        hashes[seq] = e["hash"]
        by_seq[seq] = e
        prev, expect = e["hash"], seq + 1
    first, last = events[0]["seq"], events[-1]["seq"]
    # the header must describe exactly these events and a head at or after the last of them
    if not head:
        return False, "The file records no head.", {}
    if rng.get("first_seq") != first or rng.get("last_seq") != last:
        return False, "The file's header says events %s to %s but it holds events %d to %d." % (rng.get("first_seq"), rng.get("last_seq"), first, last), {}
    if rng.get("head_seq") != head["seq"] or rng.get("head_hash") != head["hash"]:
        return False, "The file's header and its head disagree.", {}
    if head["seq"] < last:
        return False, "The file holds event %d, past the head it records (%d)." % (last, head["seq"]), {}
    if head["seq"] == last and head["hash"] != hashes[last]:
        return False, "The newest event in the file is not the head it records.", {}
    if not doc.get("truncated") and not doc.get("to") and head["seq"] != last:
        # an export with no end date runs to the head it read first
        return False, "The export ends at event %d but its own head is event %d." % (last, head["seq"]), {}
    for cp in doc.get("checkpoints") or []:
        if cp["seq"] in hashes and hashes[cp["seq"]] != cp["hash"]:
            return False, "The checkpoint at event %d does not match the event." % cp["seq"], {}
    retention_from = anchor["seq"] if anchor.get("kind") == "retention" else None
    for cp in kept:
        seq = cp["seq"]
        if seq in hashes:
            if hashes[seq] != cp["hash"]:
                return False, "Your checkpoint for event %d does not match the event in this file." % seq, {}
            continue
        if seq > head["seq"]:
            return False, ("The log's head was event %d when the file was made, but you hold a checkpoint for event %d: "
                           "newer events are missing, or the log was wound back." % (head["seq"], seq)), {}
        if seq == head["seq"]:
            if head["hash"] != cp["hash"]:
                return False, "Your checkpoint for event %d does not match the head the file records." % seq, {}
            continue
        if seq > last:
            if not partial:
                return False, ("The export ends at event %d but you hold a checkpoint for event %d (the head was %d). "
                               "If this file deliberately covers only part of the log, rerun with --partial." % (last, seq, head["seq"])), {}
            notes.append("Your checkpoint for event %d is after this file's range (not checked: --partial)." % seq)
            continue
        # before the first event
        if retention_from is not None and seq <= retention_from:
            notes.append("Your checkpoint for event %d is older than the retention cut at event %d." % (seq, retention_from))
            continue
        if not partial:
            return False, ("The export starts at event %d but you hold a checkpoint for event %d: older events are missing. "
                           "If this file deliberately covers only part of the log, rerun with --partial." % (first, seq)), {}
        notes.append("Your checkpoint for event %d is before this file's range (not checked: --partial)." % seq)
    summary = "Events %d to %d of %s verify (%d events, head %d)." % (first, last, ledger, len(events), head["seq"])
    return True, " ".join([summary] + notes), by_seq


def verify_review(review, by_seq):
    """A completed access review export against the verified log events."""
    result = review.get("result")
    if not isinstance(result, dict) or result.get("format") != "eunomia-access-review":
        return False, "This is not a completed access review export."
    if sha256_hex(canonical_json(review["snapshot"])) != review["snapshot_hash"]:
        return False, "The review's snapshot was changed: it no longer matches its hash."
    if result.get("snapshot_hash") != review["snapshot_hash"]:
        return False, "The review's result names another snapshot."
    result_hash = sha256_hex(canonical_json(result))
    if result_hash != review["result_hash"]:
        return False, "The review's decisions were changed: they no longer match its result hash."
    link = review.get("audit_event") or {}
    seq = link.get("seq")
    if seq is None:
        return False, "The review export does not name its completion event in the log."
    event = by_seq.get(seq)
    if event is None:
        return False, "The log export does not hold event %s, the review's completion." % seq
    if event["action"] != "access_review.complete" or event["target"] != result.get("review_id"):
        return False, "Event %d is not this review's completion." % seq
    if link.get("hash") != event["hash"]:
        return False, "The review export names event %d with another hash." % seq
    detail = link.get("detail")
    if not isinstance(detail, str) or sha256_hex(detail) != event["detail_digest"]:
        return False, "The completion event in the log does not commit to the detail the review export shows."
    if ("result=%s" % result_hash) not in detail.split():
        return False, "The completion event in the log records another result: the review was changed after it was completed."
    return True, "Access review %s is the one the log recorded at event %d." % (result.get("review_id"), seq)


def main(argv):
    if len(argv) < 2 or argv[1] in ("-h", "--help"):
        print(__doc__.strip())
        return 2
    kept = []
    partial = expect_empty = False
    review_path = None
    args = argv[2:]
    while args:
        if args[0] == "--partial":
            partial, args = True, args[1:]
        elif args[0] == "--expect-empty":
            expect_empty, args = True, args[1:]
        elif args[0] == "--review" and len(args) >= 2:
            review_path, args = args[1], args[2:]
        elif args[0] == "--checkpoint" and len(args) >= 2 and ":" in args[1]:
            seq, digest = args[1].split(":", 1)
            kept.append({"seq": int(seq), "hash": digest.strip().lower()})
            args = args[2:]
        else:
            print("Unknown argument: %s" % args[0], file=sys.stderr)
            return 2
    docs = []
    for path in [argv[1]] + ([review_path] if review_path else []):
        try:
            with open(path, encoding="utf-8") as f:
                docs.append(json.load(f))
        except (OSError, ValueError) as e:
            print("Cannot read %s: %s" % (path, e), file=sys.stderr)
            return 2
    try:
        ok, message, by_seq = verify(docs[0], kept, partial, expect_empty)
        if ok and review_path:
            ok, more = verify_review(docs[1], by_seq)
            message = message + " " + more if ok else more
    except (KeyError, TypeError) as e:
        ok, message = False, "The file is not a complete export (%s)." % e
    print(("OK: " if ok else "BROKEN: ") + message)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
