# Release security evidence

This is supply chain evidence for C16 and C20. It is not a security certification.
Evidence IDs below are provisional until PR01 merges its catalogue:

| Evidence ID | Evidence |
| --- | --- |
| EV-C16-1 | `Supply chain security` dependency, licence and secret reports for the exact commit |
| EV-C16-2 | Built-image CycloneDX SBOMs and Grype CVE reports for all four runtime images |
| EV-C20-1 | Release-workflow provenance attestations for each architecture and multi-arch manifest digest |
| EV-C20-2 | Signed CycloneDX SBOM predicates for those same digests |

## What the security job checks

The job runs on every PR, pushes to main and Mondays at 03:23 UTC. PR jobs have
read-only repository permissions and cannot publish images or obtain signing tokens.
The existing CI job supplies Rust Clippy and frontend ESLint/type checks.

- cargo-deny 0.20.2 checks the locked all-features backend dependency graph against
  RustSec, the licence allowlist, duplicate/wildcard policy and source allowlist.
  Duplicate versions are reported as warnings; wildcards and unknown sources fail.
- Bun 1.3.14 audits every frontend locked dependency, including development tools.
  npm on Node 24.22.2 audits the secrets-helper lockfile without installing packages.
  All audit severities block unless explicitly accepted.
- Gitleaks 8.30.1 scans the tracked working-tree snapshot, including dotfiles.
  It does not scan Git history. The negative-fixture directory alone is excluded.
  Reports use full redaction. Existing deliberately public test keys and examples
  have individual scoped acceptances, rather than directory-wide exclusions.
- Syft 1.57.0 inventories each built amd64 image. CycloneDX covers OS packages,
  frontend packages and Bun, helper packages and Node, and backup packages and
  OpenSSL. The backup SurrealDB tool records the
  version declared by the Dockerfile and its path in the image. Its embedded Rust dependency graph is unknown.
  The backend adds a labelled Cargo.lock inventory because an uninstrumented Rust
  binary cannot reliably expose its dependencies. This is a conservative superset,
  including development and target-specific dependencies, not proof that every
  listed crate is present in the final executable.
- Grype 0.121.2 scans those inventories. Unaccepted High and Critical findings
  fail, including findings without a fix. Lower and Unknown severities remain in
  the report for triage. Database download or report errors fail the job.
- actionlint 1.7.12 checks all workflow files. Real negative fixtures prove that
  a vulnerable dependency is rejected and a fake token is caught without printing it.

Tools are MIT or Apache-2.0 licensed. `scripts/ci/security/tools.json` records
versioned release archives and reviewed SHA-256 hashes. The installer checks these
hashes before extracting an executable. Workflow actions use full commit SHAs with
version comments. Advisory databases deliberately refresh, so results can change
without a source change. Reports record scanner database metadata where supplied.

Artifacts named `security-dependencies-<sha>` and `security-image-<image>-<sha>`
last 30 days. Retain them outside Actions for longer audit periods. Artifact names
on PR runs use GitHub's tested merge SHA; confirm the run's `head_sha` as well.
No application or tenant data is collected. Never upload an unredacted secret report.

## Time-limited findings

`security-acceptances.json` is the only acceptance registry. Each entry needs a
scanner, finding ID, package, scope, reason, owner and ISO expiry date. All jobs
validate the whole file. An entry expires at the start of its expiry date in UTC,
including when its finding is no longer detected. Delete resolved entries.
Acceptances match the package and scope exactly. Cargo and image entries also
include the installed version in the package key. Secret entries bind the rule,
file, line and SHA-256 of the full source file, so a changed credential cannot reuse
an old acceptance. Audit entries bind the advisory, package and project lockfile.

The initial dependency findings expire on 16 November 2026. This short acceptance
allows the evidence pipeline to land without combining runtime upgrades into it.
It is not a claim that the advisories are harmless. Cashara maintainers own
remediation. In particular:

- PR #201 upgrades Next.js and eslint-config-next together to 16.3.8 for the
  earlier advisories, including the next/og ImageResponse RCE fixed in 16.3.6.
  Retain their 16 November acceptances until it merges. The newer
  GHSA-p3m5-59mp-jv5r, GHSA-2464-fhr4-w9xw and GHSA-673q-mjm6-r5xr findings
  require 16.4.1, so PR #201 alone does not clear every current Next.js finding.
- The CI repair bumps brace-expansion to 1.1.21/5.0.12, sharp to 0.35.5 and
  source-map-js to 1.2.2 in the frontend lockfile. It also applies the available
  apt and zlib security updates in the frontend runtime base. Refresh js-yaml,
  minimatch and zod through their parent packages separately.
- Update ammonia, rustls, rustls-webpki, tokio and webpki roots to the versions
  named in each RustSec acceptance. Check crypto-bigint, quick-xml and rsa parent
  upgrades separately because their fixes cross compatibility ranges.
- SurrealDB's reblessive advisory currently requires a 4.x upgrade. Plan that
  separately with migration and old-updater tests. Replace or obtain supported
  parent dependencies for bincode, pin-utils and ttf-parser.
- Review the existing SurrealDB BUSL licence and missing crate licence metadata,
  plus the project's missing Cargo licence field. The temporary metadata
  acceptances do not grant distribution rights or approve a proprietary dependency.

The registry contains every exact finding ID and proposed solution from the first
scan. Existing fake credentials expire on 9 December 2026 and need periodic review.
The first image scan in run 42589471950 also exposed findings that had no
image-scoped acceptances. The CI repair records 235 exact advisory/package/version/
image acceptances, expiring on 16 November, after the selected frontend patches:
110 backend, 67 frontend, 51 backup and 7 secrets-helper. Most OS findings have
no fixed version in the current Debian suite according to that scan. Their
remediation needs upstream fixes or a tested base-image migration. The helper's
findings include npm's bundled brace-expansion/undici, outside its application
lockfile, and an Alpine zlib 1.3 to 1.4 update. The backend inventory also includes
SurrealDB 2.7.0 from `backend/fuzz/Cargo.lock`; its runtime locks 3.3.5. Keep those
fuzz findings visible until the fuzz target is updated and validated. Each entry
records the scanner's advisory source and proposed follow-up. Acceptance is not
proof of non-exploitability.

New container findings must receive their own scoped acceptance and proposed fix;
there is no blanket CVE suppression. Keep residual low-severity findings in triage.

The original CI pin also changed Rust from main's tested stable 1.99.1 to 1.95.0,
which overflowed the stack in the MCP prompts/resources test in both backend
jobs. The CI repair pins 1.99.1 with the same action SHA and leaves the test suite
and flags intact. Grouped shell redirects retain the same e2e environment values
while satisfying actionlint's ShellCheck checks.

## What the release workflow adds

The existing `ghcr.io/cashara-io/eunomia-{backend,frontend,backup,secrets-helper}`
repositories retain `vX.Y.Z-amd64`, `vX.Y.Z-arm64`, `vX.Y.Z` and `latest` tags.
Per-architecture images stay plain manifests. Signed attestations are attached as
OCI referrers and stored with GitHub, so their creation does not change the image
digest. The multi-arch SBOM combines inventories from both child digests selected
from that exact manifest. Its source labels distinguish architecture inventories.
The merged SBOM is a component inventory; it does not claim a dependency graph.

GitHub's `actions/attest` 4.2.2 creates SLSA v1 provenance and CycloneDX SBOM
attestations using the repository workflow's OIDC identity. No long-lived signing
key is added. The release jobs alone receive `id-token: write` and
`attestations: write`. Both tag pushes and manual rebuilds must execute at the
selected tag, with checkout HEAD equal to the workflow source SHA. For a manual
rebuild select that tag as the workflow ref, for example:

```sh
# Release operator only. Do not run this merely to test this PR.
gh workflow run release.yml --repo Cashara-io/eunomia --ref vX.Y.Z -f tag=vX.Y.Z -F latest=false
```

PR02 does not force updater verification. PR06 supplies the enforced bridge.
Release publishing still needs its existing release-order and update-path gates.
It does not automatically wait for the independent security workflow, so release
operators must review the exact commit's security results before tagging.
Historical tags without these attestations cannot pass the verification below.

## Verify a release and retrieve its signed SBOM

Install Docker with Buildx, `jq` and a current GitHub CLI with `gh attestation`.
Authenticate `gh` and, where required, Docker to GHCR with read-only access.
Set a real release tag and the independently reviewed commit SHA. These commands
verify all four manifests and both architecture digests. A tag is resolved once;
all subsequent checks use immutable digests.

```sh
set -euo pipefail
TAG=vX.Y.Z
SOURCE_SHA=REPLACE_WITH_REVIEWED_40_CHARACTER_COMMIT_SHA
REPO=Cashara-io/eunomia
WORKFLOW=Cashara-io/eunomia/.github/workflows/release.yml
mkdir -p verified-sboms
for name in backend frontend backup secrets-helper; do
  image="ghcr.io/cashara-io/eunomia-$name"
  digest="$(docker buildx imagetools inspect "$image:$TAG" --format '{{.Manifest.Digest}}')"
  docker buildx imagetools inspect "$image@$digest" --raw > "verified-sboms/$name.manifest.json"
  mapfile -t children < <(jq -er '.manifests[] | select(.platform.os == "linux" and (.platform.architecture == "amd64" or .platform.architecture == "arm64")) | .digest' "verified-sboms/$name.manifest.json")
  test "${#children[@]}" -eq 2
  for subject in "$digest" "${children[@]}"; do
    gh attestation verify "oci://$image@$subject" \
      --repo "$REPO" --signer-workflow "$WORKFLOW" \
      --source-ref "refs/tags/$TAG" --source-digest "$SOURCE_SHA" \
      --cert-oidc-issuer https://token.actions.githubusercontent.com \
      --deny-self-hosted-runners --predicate-type https://slsa.dev/provenance/v1
    gh attestation verify "oci://$image@$subject" \
      --repo "$REPO" --signer-workflow "$WORKFLOW" \
      --source-ref "refs/tags/$TAG" --source-digest "$SOURCE_SHA" \
      --cert-oidc-issuer https://token.actions.githubusercontent.com \
      --deny-self-hosted-runners --predicate-type https://cyclonedx.org/bom \
      --format json > "verified-sboms/$name-${subject#sha256:}.verification.json"
    jq -e '.[0].verificationResult.statement.predicate | select(.bomFormat == "CycloneDX")' \
      "verified-sboms/$name-${subject#sha256:}.verification.json" \
      > "verified-sboms/$name-${subject#sha256:}.cdx.json"
  done
done
```

Run this block with Bash 4 or newer, for `mapfile` support. The provenance check
validates the signature, digest, expected publisher, workflow, source tag and source
commit. The second check separately validates the signed SBOM predicate for that
same subject. Use `--bundle-from-oci` on both commands to verify referrers directly
instead of querying GitHub's attestation store. Never trust an unsigned SBOM merely
because it sits beside an image. A wrong source SHA, signer workflow or image digest
must fail. A modified standalone SBOM is not the verified predicate extracted above.

A valid signature proves who made a claim about those bytes. It does not prove the
build was reproducible, the dependencies were safe, the SBOM exhaustive or the
workflow uncompromised. Container scans do not assess a customer's running host.
Mutable Docker base tags, apt repositories and nested action downloads still need
reproducible-build input pinning in the container-hardening lane. No claim of SLSA
Level 3 is made here.

## Local proof and remaining release proof

```sh
df -h /                         # stop local scanning below 6 GB free
pgrep -x cargo                  # wait for another Cargo build to finish
python3 scripts/ci/security-tools.py cargo-deny gitleaks actionlint
bash scripts/ci/security-cargo.sh
python3 scripts/ci/security-scan.py bun frontend evidence/frontend-audit.json
python3 scripts/ci/security-scan.py npm secrets-helper evidence/helper-audit.json
python3 scripts/ci/security-scan.py gitleaks repo evidence/gitleaks-redacted.json
python3 scripts/ci/security-test.py
actionlint
bash scripts/ci/check-update-path-rows.sh
```

Release attestation validation in this PR is actionlint plus inspection against the
pinned action's inputs and GitHub CLI documentation. No tag was pushed and no release
workflow was run. A real release is still needed to prove OIDC issuance, GHCR and
GitHub attestation permissions, OCI referrer storage, both architectures, signature
verification, and rejection with the wrong identity or digest. Preserve the first
release run and verification output as EV-C20 evidence.

The common update gate remains the existing CI matrix from v2.0.7, v2.0.8, v2.0.9,
v2.1.0, v2.1.1 and v2.2.0, plus any newer published version. Keep the SurrealDB 2.x
upgrade job. This PR changes no schema, updater or image tag contract. Those facts
alone do not prove an old installation updates successfully; retain exact-SHA CI
update-path and restore results before merging.

## Implementation sources

- [GitHub artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)
- [Pinned attestation action inputs](https://github.com/actions/attest/blob/1e69f48acb82d1966a394da916b4c1698aa569d6/action.yml)
- [GitHub CLI verification policy](https://cli.github.com/manual/gh_attestation_verify)
- [cargo-deny configuration](https://embarkstudios.github.io/cargo-deny/checks/cfg.html)
- [Syft](https://github.com/anchore/syft/tree/v1.57.0), [Grype](https://github.com/anchore/grype/tree/v0.121.2) and [Gitleaks redaction](https://github.com/gitleaks/gitleaks/tree/v8.30.1)

The compliance plan's ASVS 5.0, SOC, ISO and Essential Eight sources motivate the
control mapping. These checks cover the narrow capabilities above, not every
requirement in those standards. No paid standards text is redistributed.
