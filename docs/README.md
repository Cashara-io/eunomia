# Eunomia docs

The core user guides are shown in this folder, the **Docs** page in the
app, and the `docs` MCP tool. The other pages are repository references for
operators and contributors.

| Doc | What's in it |
|---|---|
| [Quickstart](quickstart.md) | Install, sign in, connect an agent — five minutes |
| [Installation](installation.md) | Every installer flag, manual install, installing as an AI agent |
| [AI agents & MCP](agents.md) | Connecting Claude Code, Codex, Hermes, …; tokens and OAuth; the tools; running without an OpenAI key |
| [Documents](documents.md) | Uploading files: what is indexed, the MCP tools, storage (SurrealDB's built-in file buckets, optionally on S3), backups |
| [Concepts](concepts.md) | Vaults, entities, memories, recall, merging, the graph and the 3D vector cloud |
| [Connectors](connectors.md) | Credentials, source options and sync setup |
| [Extensions](extensions.md) | Your own scripts feeding Eunomia: a webhook inbox that buffers events, batch claim/ack, the security model, and the plan for hosted runtimes |
| [Deployment](deployment.md) | TLS, production env vars, updates, org databases, backups |

## Repository references

| Doc | What's in it |
|---|---|
| [Underlying workings](architecture.md) | Source-linked architecture, tenancy, jobs, ingestion, retrieval and access checks |
| [Upgrading to SurrealDB 3](upgrading-to-surrealdb-3.md) | The 1.x to 2.0 database upgrade, rollback and recovery |
| [Org tenancy](architecture/tenancy.md) | The control database, a database per org, provisioning, the one-time move and the isolation proof |
| [Background jobs](architecture/jobs.md) | The job queue, leases, workers, the scheduler leader and reconcilers |
| [SurrealQL functions](architecture/surreal-functions.md) | The SurrealQL functions the backend uses, behind the hardened server's allow-list |
| [Foundation plan](architecture/foundation-plan.md) | The 2.0 design, its implementation status and open questions |
| [Errors](errors.md) | Stable error codes and the problem+json format |
| [Debugging](debugging.md) | Trace ids, failure capsules and `replay` |
| [Observability](observability.md) | Optional OpenTelemetry export and the SigNoz profile |
| [Testing](testing.md) | Every test layer and what CI runs |
| [Releasing](releasing.md) | The SurrealDB 3 bridge rule and cutting releases |
| [SurrealQL 3.x cheat sheet](surrealql-3.md) | Writing and reviewing SurrealDB 3.x queries |
