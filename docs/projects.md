# Projects

A Eunomia project ties a repo to one vault. A small `.eunomia/` folder, committed to git,
names the server and the vault, so every agent on the team that works in the repo reads and
writes the same memory and finds the same skills.

## Set one up

In the repo's root, with your Eunomia server running:

```bash
npm create eunomia
bun create eunomia
pnpm create eunomia
```

All three run the `create-eunomia` package (its source is in [`cli/create-eunomia`](../cli/create-eunomia)).
It asks for:

1. **The server URL.** Default `http://localhost:8001`, or `EUNOMIA_URL`. It checks `/healthz` first.
2. **Sign-in.** Your email and password (typed hidden), or an existing API token. A server set to
   single sign-on only (`AUTH_MODE=oidc`) has password sign-in off: create a token in the app under
   **Settings, API tokens** and pass `--token`.
3. **The vault.** Pick one of yours or create one (the default name is the folder's name).
4. **Agents.** Optionally connects your agents with [`connect-agents.sh`](agents.md#connecting-agents),
   which gives each agent its own token. Without bash (Windows) it prints the manual steps.

It then mints a token for this project only, with the `memory:write` scope and restricted to the
project's vault, and stores it in `~/.config/eunomia/projects/` (mode 600). No token or password is
ever written into the repo. Each teammate runs the same command in their clone and signs in with
their own account; share the vault with them first (**Vaults, Members**).

Without questions, for scripts:

```bash
EUNOMIA_PASSWORD='...' npm create eunomia -- --url http://localhost:8001 --email you@example.com --vault Acme --agents none --yes
npm create eunomia -- --url https://eunomia.example.com --token <token> --vault Acme --yes
```

Running it again is safe. It keeps the files that are there, reuses your stored token while it still
works, and asks before changing the vault (`--vault` changes it without asking).

## The folder

```
.eunomia/
  eunomia.toml      server URL, vault id and name (commit it)
  skills/           one SKILL.md-style file per skill (commit them)
    README.md       how to write one (not synced)
  .gitignore        keeps a future local cache out of git
```

## Skills sync

Each `.eunomia/skills/<file>.md` is a skill:

```markdown
---
name: deploy-rollback
description: Roll back a bad deploy of this service. Use when a release breaks production.
---

1. Find the last good tag.
2. Redeploy it.
```

`name` is lowercase letters, digits and hyphens, at most 64 characters; without one the file name is
used. `description` (when to use the skill) is required. Save them into the project's vault with:

```bash
npx create-eunomia sync      # or: bunx create-eunomia sync, pnpm dlx create-eunomia sync
```

Each skill is reported as `created`, `updated`, `unchanged` (not saved again) or `error` (a bad file
is reported and the rest still sync). Skills that are in the vault but not in the folder are listed
and left alone: sync never deletes. Agents find the skills with the `skill_find` and `skill_get` MCP
tools. In CI, set `EUNOMIA_TOKEN` instead of signing in.

## Coming next

- A change log of what agents learned in the project.
- `eunomia watch`: live updates over WebSocket.
- A project ontology: the entity kinds the vault uses.
