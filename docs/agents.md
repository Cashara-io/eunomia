# AI agents & MCP

Eunomia is an MCP server (Streamable HTTP) at `http://localhost:8001/mcp`
(also proxied at `http://localhost:3000/mcp`). The backend port listens on this machine only; agents on other machines use the frontend URL, e.g. `https://eunomia.example.com/mcp` (see [deployment](deployment.md#1-https)). It exposes the same tools the
in-app chat uses. Every call is scoped to the token's user and logged in the
audit log if it changes anything.

## Connecting agents

The installer does this for you. To do it again, or for an agent you install
later:

```bash
cd eunomia
./scripts/connect-agents.sh --email you@example.com --password '…'          # every agent
./scripts/connect-agents.sh --token <token> --agents claude,codex           # an existing token
```

| Agent | Config written |
|---|---|
| Claude Code | `~/.claude.json` → `mcpServers.eunomia` |
| Codex | `~/.codex/config.toml` → `[mcp_servers.eunomia]` |
| Hermes | `~/.hermes/config.yaml` → `mcp_servers.eunomia` |
| Gemini CLI | `~/.gemini/settings.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| VS Code | `…/Code/User/mcp.json` → `servers.eunomia` |
| Claude Desktop | `…/Claude/claude_desktop_config.json` (through `npx mcp-remote`, needs Node) |

- Each agent gets its own token (Settings → API tokens, named after it), so
  you can revoke one without touching the others. Re-running replaces the
  entry and revokes the old token.
- Configs for agents you haven't installed yet are written too, so they're
  connected the first time they run (`--detected-only` turns that off).
- Other settings in those files are preserved. A file the script can't
  parse, or one with its own hand-written `eunomia` entry, is left untouched
  and reported.
- ChatGPT and Claude.ai (web/desktop) need a public `https://` URL. Add them
  by hand (below).

## Connecting one agent by hand

The onboarding wizard's **Connect your AI** step, and the **Connect an MCP
client** card on the dashboard, do this for you: pick the client, generate a
token named after it, and copy the command or config with your URL and token
already filled in. The steps it shows are these. Use the address you open
Eunomia at, plus `/mcp` (for example `http://localhost:3000/mcp`, or
`https://eunomia.example.com/mcp`). A `localhost` or LAN address only works
for agents on that computer or network. Agents learn when to recall and what
to remember from the server itself, so there is nothing else to install.

### Claude Code

```bash
claude mcp add --transport http --scope user eunomia https://eunomia.example.com/mcp \
  --header "Authorization: Bearer <token>"
```

`--scope user` makes it available in every project (the default, `local`, is
the current project only). Check it with `claude mcp list` (eunomia shows
`✔ Connected`), restart any open session, and `/mcp` inside Claude Code lists
it. To sign in with OAuth instead of a token, leave out `--header`, then run
`/mcp` in a session (or `claude mcp login eunomia`) and approve the consent
page; see [OAuth](#sign-in-with-oauth-no-pasted-token). Use one or the other:
with a header set, Claude Code doesn't fall back to OAuth.

### Codex

Add to `~/.codex/config.toml` (the Codex CLI, IDE extension and app share it):

```toml
[mcp_servers.eunomia]
url = "https://eunomia.example.com/mcp"
http_headers = { Authorization = "Bearer <token>" }
```

To keep the token out of the file, use
`bearer_token_env_var = "EUNOMIA_TOKEN"` instead of `http_headers` and export
`EUNOMIA_TOKEN` where Codex runs, or add it from the shell with
`codex mcp add eunomia --url https://eunomia.example.com/mcp --bearer-token-env-var EUNOMIA_TOKEN`.
A raw `bearer_token` key is rejected for HTTP servers. For OAuth, run
`codex mcp add eunomia --url …` with no token and `codex mcp login eunomia`.
Check it with `codex mcp list`, then `/mcp` in a new Codex session.

### ChatGPT

ChatGPT connects from OpenAI's servers, so Eunomia needs a public `https://`
address (Settings → HTTPS, or a tunnel), and `PUBLIC_URL` must be that address
(the HTTPS setup sets it). A `localhost` or LAN install can't be reached.

1. In ChatGPT on the web, open [chatgpt.com/plugins](https://chatgpt.com/plugins),
   choose **+**, then **Add custom MCP server**.
2. Name it Eunomia, enter the server URL including `/mcp`, and set
   authentication to **OAuth** (ChatGPT has no option for a pasted bearer
   token; it registers itself with Eunomia's OAuth server).
3. Select **I understand and want to continue**, then **Create as a plugin**,
   and approve access on Eunomia's consent page.
4. Install it from your personal plugins, then in a chat type `@` and pick
   Eunomia.

Custom MCP servers depend on your ChatGPT plan and workspace policy: OpenAI
lists Business, Enterprise and Edu with full (read and write) access, and Pro
with read-only tools, which can `recall` but not save. Sources:
[OpenAI: connect ChatGPT](https://developers.openai.com/plugins/deploy/connect-chatgpt),
[developer mode and MCP apps](https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt).

## The memory skill

The **Memory skill** page (sidebar) holds the instructions that tell agents
when to recall and what to remember. You can view it, edit it, or reset it to the
built-in default. It reaches agents three ways:

- **Every MCP client** gets it as the server's connection instructions, so an
  edit applies the next time the agent connects.
- **Claude Code, Codex, Hermes** get a `skills/eunomia-memory/SKILL.md`
  (re-run `connect-agents.sh` to refresh the file copies).
- **Claude Code** also gets two hooks in `~/.claude/settings.json`:
  `SessionStart` injects your current skill, and `UserPromptSubmit` calls
  `recall` with each prompt and injects the relevant memories before Claude
  sees it. The hooks are silent and never block a prompt if Eunomia is down.

By hand, for any MCP client: URL `http://localhost:8001/mcp`, header
`Authorization: Bearer <token>`. Create a token in Settings → API tokens.
This is the fallback that works with every client.

### Sign in with OAuth (no pasted token)

MCP clients that support OAuth (Claude Code, Cursor, and others following the
MCP authorization spec, revision 2026-07-28) need only the URL:

```bash
claude mcp add --transport http eunomia http://localhost:8001/mcp
```

The client discovers Eunomia's authorization server, opens your browser, and
shows a consent page: who is asking, where you return to afterwards, and what
it may do (read memory, write memory, manage vaults, manage sources). Allow
it and the client holds a 15-minute access token plus a refresh token it
rotates on its own. Nothing is pasted.

- **Manage apps**: Settings → Connected apps lists every app you approved.
  Disconnecting one revokes its tokens immediately.
- **Scopes**: `memory:read`, `memory:write`, `vaults:admin`, `connectors`. A
  token that only has `memory:read` is refused (HTTP 403, `insufficient_scope`)
  when it calls a tool that writes. The default request is read plus write.
- **Bound to this server**: tokens are issued for the MCP URL only, work on
  `/mcp` only, and are rejected anywhere else.
- **Public URL**: set `PUBLIC_URL` (default `http://localhost:8001`) to the
  address clients reach Eunomia at, with no trailing slash. It becomes the
  token audience and the base of the discovery documents, so set it to
  `https://eunomia.example.com` when you put Eunomia behind a domain. Behind
  the frontend on port 3000, use `http://localhost:3000`; `/.well-known/*` and
  `/oauth/*` are proxied there too. Changing it invalidates existing tokens.
- **Client registration**: clients identified by an HTTPS metadata URL (Client
  ID Metadata Documents) work without registering; older clients register
  through `/oauth/register`. Eunomia fetches metadata documents only from
  public HTTPS hosts. A self-hosted install with no internet access can still
  use clients that register dynamically.
- **Discovery**: `/.well-known/oauth-protected-resource` and
  `/.well-known/oauth-authorization-server`. An unauthenticated `/mcp` call
  answers `401` with a `WWW-Authenticate: Bearer resource_metadata="..."`
  header pointing at the first.
- **Not supported**: confidential clients (client secrets), the implicit and
  password grants, `plain` PKCE.

## Do I need an OpenAI key?

No. The connected agent is the model.

| Without a key | With a key (recommended to save tokens) |
|---|---|
| `recall`: keyword + full-text over memories + graph + recency | adds semantic (embedding) search |
| `reflect` returns the numbered memories; your agent writes the answer | Eunomia writes the cited answer |
| `consolidate_observations`: your agent writes the belief with `memory_write` type `observation` | Eunomia synthesizes it |
| Vector cloud uses a lexical (word) space | Real embedding space |
| In-app Chat is off | In-app Chat works |

## Tools

| Tool | Does |
|---|---|
| `docs` | Read these docs. No args lists them, `topic` reads one |
| `recall` | Best memories and records for a question, within a token budget |
| `reflect` | Cited answer from recalled memories (or the memories, without a key) |
| `search` / `get` / `list` / `links` | Raw synced records from connectors |
| `memory_write` | Remember a fact (`world`, `experience`) or set an entity's `observation` |
| `memory_update` / `memory_delete` | Edit or delete one memory |
| `entities_search` / `entities_get` / `entities_graph` | Find and read people, orgs, places, code |
| `entity_update` / `entity_merge` / `entity_delete` | Edit, de-duplicate, delete entities |
| `code_entity_upsert` / `code_relate` | Map repositories, files, symbols and their relations |
| `consolidate_observations` | Fold new facts into each entity's belief |
| `vault_list` / `vault_create` / `vault_clone` / `vault_merge` | Manage vaults (separate scopes for projects, clients, teams, …); merge two into a new one. Any tool's `vault_id` also accepts a vault's name |
| `vault_invite` / `vault_members` / `vault_remove_member` / `vault_leave` / `vault_rename` / `vault_delete` | Sharing |
| `document_upload` / `document_list` / `document_get` | Upload a document (text, Markdown, JSON, CSV, text PDF) and read its extracted text; its passages are found by `search`, `recall` and `get`, each hit naming the document ([documents](documents.md)) |
| `document_download` / `document_export` / `document_delete` | The HTTP route for the original bytes, the passages-and-vectors manifest, deletion |

Destructive tools are flagged `destructiveHint` so clients can ask first.
