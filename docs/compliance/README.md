# Self-hosted readiness catalogue

This catalogue describes Eunomia's source at the commit recorded in [controls.json](controls.json). It does not establish deployed effectiveness or independent assurance. There are 36 capabilities: 5 Done, 23 Partial and 8 Missing. A capability can support several clauses without satisfying any whole clause.

Start with [responsibilities](responsibilities.md) and [applicability](applicability.md). The [ASVS checklist](asvs-checklist.md) covers every L1/L2 requirement separately. Organisational work belongs to the [policy track](policy-track.md).

## Reading the data

`catalogue_version` versions the catalogue contract. Capability IDs C01 to C36 are stable. Do not recycle an ID when a capability changes. Each row records its implementation finding, owner, code/documentation track, clause mappings, evidence and a future observable acceptance check. Those acceptance checks are work to do, not results of this PR.

Standards have an edition, sources, review date and clause registry. ISO/IEC 27001, ISO/IEC 42001, CIS and SOC 2 mappings are `provisional` until checked against authorised text. Other crosswalks are public-source proposals, not independent verification. The plan's ASVS section ranges have been expanded to actual 5.0.0 requirement IDs. Some supporting crosswalk entries are L3; they do not enlarge the L2 assessment target. Clause text from restricted standards is not included.

Evidence records use [evidence-schema.json](evidence-schema.json), JSON Schema draft 2020-12. `EV-<capability>-<positive integer>` is the agreed producer contract, for example `EV-C20-2`. IDs are unique within a scoped evidence set. `EV-Cxx-1` is reserved for the baseline source inventory. PR02 and later producers start at 2, coordinate allocations and never recycle an ID. Repeated captures retain release SHA and capture time, with history preserved. Each record points to one capability. An artefact supporting several capabilities can have separate records pointing to the same artefact.

When the snapshot moves, every capability gets a new record captured at the new commit, and the commit it replaces is added to `snapshot.previous_commits`. Earlier records are not edited: they keep their own release SHA and stay as history. A row's newest record (the highest number) must be at the snapshot and state the row's status. An earlier record may state an earlier status.

Repository evidence is a path and inclusive line range at `subject.release_sha`. CI checks paths and ranges against the checked-out tree for records at the snapshot only; an earlier record resolves at its own SHA through its immutable link. The inventory IDs E01 to E27 retain the plan's reference names; they are not new measured checks. Build an immutable source link as `https://github.com/Cashara-io/eunomia/blob/<release_sha>/<path>#L<line_start>`. External historical evidence has its own immutable URL. E27 is an imported historical failure, not a new release verdict. These references were inspected; the named product tests were not run for this documentation change.

Keep three fields separate:

| Field | Values and meaning |
|---|---|
| Implementation | Done, Partial, Missing. These are bounded source findings. |
| Operation | passes measured check, fails measured check, unknown, stale, not applicable with reason and approver. |
| Source | Eunomia, imported signed evidence, customer attestation. A customer statement is not a measured pass. |

Measured results need a check ID, check time and freshness deadline. Schema examples are illustrative, including a deliberate failed check. They are not operating evidence. The baseline records use `operation: unknown`. A schema-valid record is only structurally valid; review its truth, signatures, scope and freshness separately.

Evidence packs in PR14 will add a manifest with org, build/image digests, migrations, capture interval, versions and file hashes. Preserve unknown/stale results, failed attempts and bounded capture watermarks. Default exports must exclude prompts, chat content, raw personal data, secrets and unrelated orgs. Keep tenant narratives in the tenant database. A checksum establishes a narrower integrity fact than truth or resistance to a host administrator. The catalogue itself implements no evidence-pack export or live Settings view.

## Checking changes

Use Python 3.11 or newer and an isolated environment:

```sh
python3 -m venv /tmp/eunomia-readiness-venv
/tmp/eunomia-readiness-venv/bin/pip install -r scripts/ci/requirements-compliance.txt
PATH=/tmp/eunomia-readiness-venv/bin:$PATH bash scripts/ci/check-compliance.sh
PATH=/tmp/eunomia-readiness-venv/bin:$PATH bash scripts/tests/check-compliance.test.sh
bash scripts/ci/check-update-path-rows.sh
```

CI's `scripts` job runs the structural check and its rejection fixtures. The existing `update-path` job retains all seven old-release rows, v2.0.7 to v2.3.0. Its real update and backup-based rollback proof remains required before merge. This PR needs no migration or Cargo build. An installed docs tree can be opened without a running backend; linked JSON and Markdown stay relative to that tree.
