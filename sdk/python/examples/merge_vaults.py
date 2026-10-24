"""Create two vaults, write facts into each, merge them and recall from the result.

    EUNOMIA_URL=http://localhost:3000 EUNOMIA_TOKEN=... python examples/merge_vaults.py

It deletes the vaults it made when it finishes. Set KEEP=1 to leave them in place.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from eunomia import Eunomia  # noqa: E402

eu = Eunomia(os.environ["EUNOMIA_URL"], os.environ["EUNOMIA_TOKEN"])
made = []
try:
    a = eu.create_vault("Research")
    b = eu.create_vault("Notes")
    made += [a.id, b.id]
    print(f"created {a.name} ({a.id}) and {b.name} ({b.id})")

    for r in eu.write_memories(
        [
            {"subject_name": "Ada", "subject_kind": "person", "text": "Ada prefers green tea"},
            {"subject_name": "Ada", "subject_kind": "person", "text": "Ada works on the analytical engine"},
        ],
        vault_id=a.id,
    ) + eu.write_memories(
        [{"subject_name": "Grace", "subject_kind": "person", "text": "Grace wrote the first compiler"}],
        vault_id=b.id,
    ):
        assert r.ok, r.error
    print("wrote 2 facts to Research and 1 to Notes")

    merged = eu.merge_vaults(a.id, b.id, name="Everything")
    made.append(merged.id)
    print(f"merged into {merged.name} ({merged.id}): {merged.entities} entities")

    for hit in eu.recall("who wrote a compiler", vault_id=merged.id, limit=3):
        print(f"recall -> {hit['text']}  [{hit['vault_name']}]")
finally:
    if os.environ.get("KEEP"):
        print("KEEP set: leaving", ", ".join(made))
    else:
        for vid in made:
            eu.delete_vault(vid)
        print(f"deleted {len(made)} vaults")
