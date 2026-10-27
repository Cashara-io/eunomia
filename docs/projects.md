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

1. **The server URL.** `--url`, `EUNOMIA_URL`, or typed at the prompt (default `http://localhost:8001`).
   It checks `/healthz` first. In a repo that already has `.eunomia/eunomia.toml`, that file's URL is
   used, but your credentials only go to it once you confirm it (or name the same URL with `--url`
   or `EUNOMIA_URL`). A named URL that differs from the file's stops init before anything is sent,
   so a committed file can never steer your password or token to another server. Plain `http://` to
   another machine gets a warning, and a URL with a user name or password in it is refused.
2. **Sign-in.** Your email and password (typed hidden), or an existing API token (checked before
   init goes on). A server set to single sign-on only (`AUTH_MODE=oidc`) has password sign-in off:
   create a token in the app under **Settings, API tokens** and pass `--token`.
3. **The vault.** Pick one of your shared vaults or create one. Your personal vault is not offered,
   since teammates cannot join it. `--vault` takes an id or a name; a name you have no vault for is
   an error unless you add `--create-vault` (interactive runs ask first), and a name two of your
   vaults share is an error that lists their ids.
4. **Agents.** Only when you ask. [`connect-agents.sh`](agents.md#connecting-agents) writes each
   agent's global config with its own new token, which has your account's full scopes and is not
   limited to this vault. Interactive runs ask (default no) and then connect only the agents that
   look installed (`--agents all --detected-only`); `--agents claude,codex` picks them yourself;
   `--yes` without `--agents` connects none and prints the command to run. The script gets your
   email and password, never a token: if you signed in with `--token`, no agent is connected and
   init prints how to do it. Without bash (Windows) it prints the manual steps, and if the script
   fails, init exits non-zero.

It then mints a token for this project only, with the `memory:write` scope and restricted to the
project's vault, and stores it in `~/.config/eunomia/projects/` (mode 600, folders 700), one file
per server URL and vault. No token or password is ever written into the repo. Each teammate runs the
same command in their clone and signs in with their own account; share the vault with them first
(**Vaults, Members**).

Without questions, for scripts:

```bash
EUNOMIA_PASSWORD='...' npm create eunomia -- --url http://localhost:8001 --email you@example.com --vault Acme --create-vault --yes
npm create eunomia -- --url https://eunomia.example.com --token <token> --vault Acme --yes
```

Running it again is safe. It keeps the files that are there, reuses your stored token while it still
works, and asks before changing the vault (`--vault` changes it without asking). A new token is
minted only when this machine has none that works; the one it replaces (by id, from this machine's
file) is revoked after the new one exists, so tokens on your other machines or CI keep working.

## The folder

```
.eunomia/
  eunomia.toml      server URL, vault id and name (commit it)
  skills/           one SKILL.md-style file per skill (commit them)
    README.md       how to write one (not synced)
  .gitignore        keeps a future local cache out of git
```

## Skills sync

Each `.eunomia/skills/<name>.md`, or `.eunomia/skills/<name>/SKILL.md`, is a skill. Only regular
files count: a symlink is reported and skipped, so sync never reads outside the repo.

```markdown
---
name: deploy-rollback
description: Roll back a bad deploy of this service. Use when a release breaks production.
---

1. Find the last good tag.
2. Redeploy it.
```

`name` is lowercase letters, digits and hyphens, at most 64 characters; without one the file name is
used (or the folder name for `<name>/SKILL.md`). `description` (when to use the skill) is required. Save them into the project's vault with:

```bash
npx create-eunomia sync      # or: bunx create-eunomia sync, pnpm dlx create-eunomia sync
```

Each skill is reported as `created`, `updated`, `unchanged` (not saved again) or `error` (a bad file
is reported and the rest still sync). Skills that are in the vault but not in the folder are listed
and left alone: sync never deletes. Agents find the skills with the `skill_find` and `skill_get` MCP
tools. In CI, set `EUNOMIA_TOKEN` and `EUNOMIA_URL` (it must match the URL in `eunomia.toml`)
instead of signing in. A token the server refuses stops the sync with one message: run init again.

## Coming next

- A change log of what agents learned in the project.
- `eunomia watch`: live updates over WebSocket.
- A project ontology: the entity kinds the vault uses.
