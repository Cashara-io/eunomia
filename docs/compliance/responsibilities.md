# Shared responsibility for self-hosters

Paperless Labs, Cashara Pty Ltd, Australia, makes Eunomia. The customer normally runs the installation and decides why it processes personal information. This readiness catalogue covers that boundary. Supplying a self-hosted licence alone does not make Cashara the GDPR processor of every customer's records. Support access, telemetry or future hosting can change the roles and need a fresh assessment.

## Who does what

| Area | Eunomia and Cashara | Customer |
|---|---|---|
| Product security | Secure development, dependency review, release integrity, vulnerability response and accurate operating instructions. | Install supported releases, assess local exposure and apply patches. |
| Identity and access | Provide and test the documented auth, tenant and vault boundaries. Record gaps honestly. | Manage people, IdP assurance, scopes, access reviews, offboarding and emergency access. |
| Infrastructure and ordinary transport | Document supported container and application transport settings and exceptions. | Protect hosts, disks, networks, TLS termination, internal links and administrator access. |
| Stored data and secrets | Implement documented secret encryption and recovery paths. Distinguish credential encryption from volume encryption. | Protect keys, backups and live volumes. Test recovery and control exports, connector destinations and support access. |
| Availability | Supply bounded operations and tested update/restore tools. | Set recovery objectives, retain isolated copies, rehearse restoration of every org and document file bytes, and monitor capacity. |
| Privacy | Supply truthful product descriptions and rights-handling building blocks. Cashara owns its own collection and incidents. | Decide lawful purposes, categories, notices, retention, rights decisions, non-model disclosures and breach notifications. |
| Audit and evidence | Keep source findings, measured checks and attestations separate. Provide evidence-producing features in later waves. | Retain operating evidence, review unknowns and stale results, approve exceptions and protect scoped records. |
| AI features made by Eunomia | Govern intended purpose, foreseeable misuse, disclosure, provenance, oversight, tool enforcement, evaluation and product incidents. | Assess actual use, train operators, review important outputs and enforce local approval and stop policies. |
| AI model endpoint | Record only the model/endpoint identifiers needed for Eunomia activity and reproducibility. | Select the endpoint. The model data path is excluded by owner decision and assumed encrypted. |

In `controls.json`, `Eunomia` means the product-maker action owner, `customer` means the deployment/operator action owner, and `shared` needs work on both sides. A product row's code and documentation track does not mean an organisational policy has been adopted. The [policy track](policy-track.md) has its own owners and approval work.

## Boundaries that stay visible

The excluded model path includes provider retention, training, residency, subprocessors, provider contracts, provider certification and provider data handling. This wave adds no provider assessment. Eunomia's own storage, chat history, connector secrets, ordinary application transport, tools and exports remain in scope. Maker duties for AI features remain in scope too.

`audit_event` is the security ledger: delivered durably within each process and sealed into a hash chain per organisation, with verification and a metadata-only export (C07). Its checkpoints share the database with the events, so nothing outside the host anchors it. Tenant `audit_log` contains tool summaries. `tool_usage` holds mutable hourly aggregates. The content-free `changes` feed is sync machinery. They are different records and none currently establishes a complete evidence pack. Chat history already stores content; a future metadata-only activity log must not hide that fact.

A narrowly Done implementation still needs deployment and operating evidence. For example, AES-GCM secret storage does not prove disk encryption. OIDC validation does not prove enforced MFA. A running backup service does not prove an isolated copy or a successful restore within the customer's recovery objective.

A future hosted edition needs a new service boundary, controller/processor analysis, availability commitments and evidence programme. The self-hosted catalogue cannot be reused as a hosted service attestation.

## Essential Eight boundaries

Select a maturity target and record actual host and endpoint evidence against the [ASD model](https://www.cyber.gov.au/publication/essential-eight-maturity-model). These eight boundaries are not eight verified product controls.

| Strategy | Product support and customer duty |
|---|---|
| Application control | Customer allows trusted applications on hosts and endpoints. Eunomia does not assess execution policy. |
| Patch applications | C16/C20 support release and dependency work. Customer deploys patches within its target deadlines. |
| Configure Microsoft Office macro settings | Customer endpoint policy. No Eunomia macro enforcement is claimed. |
| User application hardening | Customer browser and endpoint policy. C10 supports only Eunomia's application settings. |
| Restrict administrative privileges | C04/C08 support application access. Customer controls privileged host accounts and reviews. |
| Patch operating systems | Customer host and endpoint inventory, support status and patch deadlines. |
| Multi-factor authentication | C02 is Partial: password accounts can use an authenticator app with recovery codes, and an operator can require it for every password account; passkeys are not built. Single sign-on accounts are left to the IdP unless `OIDC_REQUIRE_MFA` is set, which trusts the IdP's `amr`/`acr` claim. Customer IdP configuration alone is unproved application assurance. |
| Regular backups | C15 supplies partial backup/restore tools. Customer owns isolation, retention, key protection and measured drills. |
