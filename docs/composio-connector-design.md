# Composio consumer connector

Implementation notes for the Composio connector lane. This document records the research and current limits. It is not a claim that the connector is available in Connected sources yet.

## Account and authentication

Use `https://connect.composio.dev/mcp`. Each person signs in to their own Composio account. Eunomia is an MCP client and does not create a developer project, provide an API key, or allocate a Composio entity for a person.

Primary sources:

- [Composio Connect](https://docs.composio.dev/docs/composio-connect) documents the shared consumer MCP URL and its meta tools.
- [Composio consumer and developer boundaries](https://docs.composio.dev/kb/guide/consumer-project-boundaries-and-auth-selection) explains that consumer connections are separate from developer projects and belong to the member who authorised them. A consumer key is a supported fallback for clients without OAuth support; a developer project key is a different credential.
- [Composio's Claude setup](https://composio.dev/claude) uses OAuth without manually supplied authentication headers.
- [MCP authorisation](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization) specifies protected resource metadata, authorisation server discovery, registration, PKCE and the resource parameter.

Live anonymous metadata checked on 9 November 2026:

- `https://connect.composio.dev/.well-known/oauth-protected-resource/mcp` identifies `https://connect.composio.dev/mcp` as the resource and `https://connect.composio.dev` as its authorisation server.
- `https://connect.composio.dev/.well-known/oauth-authorization-server` advertises `https://connect.composio.dev/oauth/authorize`, public client registration at `https://login.composio.dev/oauth2/register`, tokens at `https://login.composio.dev/oauth2/token`, PKCE S256, token endpoint authentication `none`, and authorisation-code and refresh-token grants. Supported scopes include `openid profile email offline_access`.
- Neither this metadata nor `https://login.composio.dev/.well-known/openid-configuration` advertises a revocation endpoint. Do not guess one or label local deletion as remote revocation. A completed connector must clearly explain the remote session removal step in Composio if no supported programmatic revocation mechanism can be established.

The OAuth primitive constrains metadata endpoints to the documented HTTPS hosts, disables HTTP redirects and bounds response sizes. It checks the resource, issuer, PKCE method and public-client support. Provider errors return a fixed message and never include response bodies, codes or credentials. Secret-bearing pending-state and token types do not implement Debug and clear their secret fields on drop.

Storage and account functions have been written for encrypted `enc:v1` state, owner-scoped conditional writes, one-use pending state, caller-supplied session binding, refresh leases and token deletion. They are not wired into HTTP routes or verified. These properties still need full integration and guard tests before they can be claimed for the connector.

The transport primitive implements MCP initialisation, session headers, JSON and bounded SSE responses. It allows only the discovery, schema, connection-management and multi-execution meta tools. Its caller must constrain execution to the approved read tools and pin the user's chosen connected account. Results are scrubbed for the bearer token and MCP session ID before they reach the source layer. Transport errors expose fixed messages and classify HTTP 401 and 429 separately.

## Source design

Use the existing `Source` contract, scheduler, per-connection cursors and vault routing. Extend the connector-kind constraint with tenant migration 0040. Store Composio tokens in the existing encrypted credentials field. Keep per-app sync choices and account IDs in connector configuration. Never expose tokens through connector output, source payloads, exported records or logs.

Before executing a read tool, discover it with `COMPOSIO_SEARCH_TOOLS`, load its complete schema with `COMPOSIO_GET_TOOL_SCHEMAS`, confirm an active connected account, and pass that specific account to `COMPOSIO_MULTI_EXECUTE_TOOL`. Several Drive accounts must have distinct checkpoints and external record IDs. Do not expose arbitrary tool execution or write operations through sync configuration.

Required adapters, still to be implemented and verified:

- Google Drive: selected files or folders, or all accessible files. Export Google Docs to text and Sheets to CSV. Download supported text files and PDFs to Eunomia documents for indexing. Retain pagination and retry incomplete file reads without advancing past them. [Drive toolkit](https://docs.composio.dev/toolkits/googledrive).
- Gmail: incremental reads, bounded batches and checkpoints that cannot skip unread messages. On 429, ingest completed reads and retain the retry position. PR #198 was open when checked, so its rate-limit work was not on the implementation base. [Gmail toolkit](https://docs.composio.dev/toolkits/gmail).
- Google Calendar: paginated event reads and incremental checkpoints. [Calendar toolkit](https://docs.composio.dev/toolkits/googlecalendar).
- Personal WhatsApp: use TimelinesAI's chat and message read tools. This requires the user's separate TimelinesAI connection and service. The `whatsapp` toolkit is for WhatsApp Business accounts and cannot read a personal WhatsApp inbox. Do not offer it as personal chat sync. [TimelinesAI toolkit](https://docs.composio.dev/toolkits/timelinesai), [WhatsApp toolkit](https://docs.composio.dev/toolkits/whatsapp).
- Obsidian: read Markdown from a user-selected GitHub repository, parse front matter and wikilinks, and retain relations. This is one-way ingestion of files already pushed to GitHub, not Obsidian Sync or live device access. A Drive-hosted vault can use the Drive Markdown path later. [GitHub toolkit](https://docs.composio.dev/toolkits/github).

## Verification remaining

The source adapters, persistence, UI, integration tests, isolation and disconnect guard mutations, Playwright run, screenshots and owner verification stack are not complete. The existing primitive mock tests are not substitutes for those checks. Do not describe the owner stack as waiting for sign-in until it has actually been launched and checked.

Implementation stopped when the disk guard observed less than 6 GiB free. Eight primitive tests passed before storage and account additions. The storage test build was interrupted. Clippy found a style warning that was fixed, but its rerun and the remaining checks have not completed. See the local handback file in the verification scratch directory for exact status.
