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
- Other settings in those files are preserved. An agent config file the
  script can't parse, or one with its own hand-written `eunomia` entry, is
  left untouched and reported.
- ChatGPT and Claude.ai (web/desktop) need a public `https://` URL. Add it
  by hand under their Connectors settings.

## The memory skill

The **Memory skill** page (sidebar) holds the instructions that tell agents
when to recall and what to remember. You can view it, edit it, or reset it to the
built-in default. The MCP server carries it, so no harness-specific setup is
needed:

- **Every MCP client** gets it as the server's connection instructions, so an
  edit applies the next time the agent connects. It is also the MCP prompt
  `eunomia-memory` (a slash command in Claude Code) and the resource
  `eunomia://skill`.
- **Clients that ignore all of those** still see the essentials in the tool
  descriptions: `recall` says to call it at the start of a task, and
  `memory_write` says to save facts, decisions, preferences and corrections
  as they come up, plus a short summary of what was learned before finishing.
  The skill's "Before you finish" step asks for the same, so a session is
  captured without any hook.
- **Claude Code, Codex, Hermes** get a `skills/eunomia-memory/SKILL.md`
  (re-run `connect-agents.sh` to refresh the file copies).
- **Claude Code hooks (optional)**: `connect-agents.sh` also adds two hooks to
  `~/.claude/settings.json`: `SessionStart` injects your current skill, and
  `UserPromptSubmit` calls `recall` with each prompt and injects the relevant
  memories before Claude sees it. The hooks are silent and never block a
  prompt if Eunomia is down. When the script can't edit that file (it isn't
  plain JSON, or the agent running the script may not change its own
  settings), it skips the hooks with a one-line note; everything else works
  without them.

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
