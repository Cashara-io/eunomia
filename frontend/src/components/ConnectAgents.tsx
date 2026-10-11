"use client";

// Setup guides for the AI clients people connect most: one tab each, a token
// minted for that client, and the exact command or config with this instance's
// URL and the token filled in. Used by onboarding and the dashboard.

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useRef, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { KeyRound } from "lucide-react";
import { useCreateToken } from "@/lib/queries/auth";
import CopyButton from "@/components/bits/CopyButton";
import DecryptReveal from "@/components/bits/DecryptReveal";

const CLIENTS = [
  { id: "claude", label: "Claude Code", token: "Claude Code" },
  { id: "codex", label: "Codex", token: "Codex" },
  { id: "chatgpt", label: "ChatGPT", token: null },
  { id: "other", label: "Other", token: "MCP" },
] as const;

type ClientId = (typeof CLIENTS)[number]["id"];

// Same origin as the app: the frontend proxies /mcp to the backend.
const noSubscribe = () => () => {};
export function useMcpUrl(): string {
  return useSyncExternalStore(noSubscribe, () => `${window.location.origin}/mcp`, () => "");
}

/** Only reachable from this computer or this network, so no cloud service (ChatGPT) can call it. */
function isPrivateHost(host: string): boolean {
  return (
    host === "localhost" ||
    host.endsWith(".localhost") ||
    host.endsWith(".local") ||
    host === "[::1]" ||
    /^127\./.test(host) ||
    /^10\./.test(host) ||
    /^192\.168\./.test(host) ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(host) ||
    /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(host) // CGNAT, e.g. Tailscale
  );
}

const claudeCodeCommand = (url: string, token: string) =>
  `claude mcp add --transport http --scope user eunomia ${url} --header "Authorization: Bearer ${token}"`;

const codexConfig = (url: string, token: string) =>
  `[mcp_servers.eunomia]\nurl = "${url}"\nhttp_headers = { Authorization = "Bearer ${token}" }`;

const otherConfig = (url: string, token: string) =>
  JSON.stringify({ mcpServers: { eunomia: { type: "http", url, headers: { Authorization: `Bearer ${token}` } } } }, null, 2);

function Code({ value, label }: { value: string; label: string }) {
  return (
    <div className="field relative px-2.5 py-2 pr-9">
      <pre aria-label={label} className="text-[12px] font-mono whitespace-pre-wrap [overflow-wrap:anywhere]" style={{ color: "var(--ink)" }}>
        {value}
      </pre>
      <CopyButton value={value} label={`Copy ${label.toLowerCase()}`} className="absolute top-1 right-1" />
    </div>
  );
}

function Step({ n, title, children }: { n: number; title: React.ReactNode; children?: React.ReactNode }) {
  return (
    <li className="grid grid-cols-[18px_minmax(0,1fr)] gap-x-2 gap-y-1.5">
      <span className="font-mono text-[12px] leading-[20px]" style={{ color: "var(--ink-faint)" }} aria-hidden>
        {n}
      </span>
      <div className="text-[13px] leading-[20px]">{title}</div>
      {children && <div className="col-start-2 flex flex-col gap-1.5 min-w-0">{children}</div>}
    </li>
  );
}

const Mono = ({ children }: { children: React.ReactNode }) => <code className="font-mono text-[12px]">{children}</code>;
const Hint = ({ children }: { children: React.ReactNode }) => (
  <p className="text-[12px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
    {children}
  </p>
);

/** `onToken` fires after a token is minted, so a wizard can turn Skip into Continue. */
export default function ConnectAgents({ onToken }: { onToken?: () => void }) {
  const mcpUrl = useMcpUrl();
  const [client, setClient] = useState<ClientId>("claude");
  // One token per client, shown once; kept while the guide stays mounted.
  const [tokens, setTokens] = useState<Partial<Record<ClientId, string>>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const createToken = useCreateToken();
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);

  const current = CLIENTS.find((c) => c.id === client)!;
  const token = tokens[client];
  const host = mcpUrl ? new URL(mcpUrl).hostname : "";
  const local = isPrivateHost(host);
  const https = mcpUrl.startsWith("https://");

  async function mint() {
    if (!current.token) return;
    setBusy(true);
    setError(null);
    try {
      const res = await createToken.mutateAsync({ name: current.token });
      setTokens((t) => ({ ...t, [client]: res.token }));
      onToken?.();
    } catch (err) {
      setError(failure(err, "Could not generate a token."));
    } finally {
      setBusy(false);
    }
  }

  function choose(id: ClientId) {
    setClient(id);
    setError(null);
  }

  function onKey(e: React.KeyboardEvent, i: number) {
    const step = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const j = (i + step + CLIENTS.length) % CLIENTS.length;
    choose(CLIENTS[j].id);
    tabRefs.current[j]?.focus();
  }

  const mintStep = (
    <Step n={1} title={token ? "Your token, shown once" : "Generate a token for " + current.label}>
      {token ? (
        <div className="field flex items-center gap-2 h-8 pl-2.5 pr-1">
          <code className="flex-1 min-w-0 truncate text-[12px] font-mono" style={{ color: "var(--ink)" }}>
            <DecryptReveal key={token} text={token} />
          </code>
          <CopyButton value={token} label="Copy token" className="shrink-0" />
        </div>
      ) : (
        <button onClick={mint} disabled={busy} aria-busy={busy} className="btn btn-primary self-start">
          <KeyRound size={14} strokeWidth={1.75} aria-hidden />
          {busy ? "Generating…" : "Generate a token"}
        </button>
      )}
      <Hint>
        Named &ldquo;{current.token}&rdquo; under{" "}
        <Link href="/settings?tab=tokens" className="underline" style={{ color: "var(--accent-text)" }}>
          Settings, API tokens
        </Link>
        , so you can revoke it on its own.
      </Hint>
    </Step>
  );

  return (
    <div className="flex flex-col gap-4">
      <div role="tablist" aria-label="AI client" className="flex flex-wrap gap-1.5">
        {CLIENTS.map((c, i) => (
          <button
            key={c.id}
            ref={(el) => {
              tabRefs.current[i] = el;
            }}
            role="tab"
            id={`connect-tab-${c.id}`}
            aria-selected={client === c.id}
            aria-controls="connect-panel"
            tabIndex={client === c.id ? 0 : -1}
            onClick={() => choose(c.id)}
            onKeyDown={(e) => onKey(e, i)}
            className="pill"
          >
            {c.label}
          </button>
        ))}
      </div>

      <div role="tabpanel" id="connect-panel" aria-labelledby={`connect-tab-${client}`} className="flex flex-col gap-4 min-w-0">
        {client === "chatgpt" ? (
          <ChatGptGuide mcpUrl={mcpUrl} reachable={https && !local} />
        ) : (
          <ol className="flex flex-col gap-4">
            {mintStep}
            {client === "claude" && (
              <>
                <Step n={2} title="Run this in a terminal">
                  {token ? <Code label="Command" value={claudeCodeCommand(mcpUrl, token)} /> : <Pending />}
                  <Hint>
                    <Mono>--scope user</Mono> makes Eunomia available in every project.
                  </Hint>
                </Step>
                <Step n={3} title="Check it">
                  <Hint>
                    Run <Mono>claude mcp list</Mono>: eunomia shows as connected. Restart any open Claude Code session,
                    then ask &ldquo;What do you remember about me?&rdquo;
                  </Hint>
                </Step>
              </>
            )}
            {client === "codex" && (
              <>
                <Step n={2} title={<>Add this to <Mono>~/.codex/config.toml</Mono></>}>
                  {token ? <Code label="Config" value={codexConfig(mcpUrl, token)} /> : <Pending />}
                  <Hint>Create the file if it isn&apos;t there. The Codex CLI, IDE extension and app all read it.</Hint>
                </Step>
                <Step n={3} title="Check it">
                  <Hint>
                    Run <Mono>codex mcp list</Mono>: eunomia is listed. In a new Codex session, <Mono>/mcp</Mono> shows its
                    tools; ask &ldquo;What do you remember about me?&rdquo;
                  </Hint>
                </Step>
              </>
            )}
            {client === "other" && (
              <>
                <Step n={2} title="Add it to your client's MCP config">
                  {token ? <Code label="Config" value={otherConfig(mcpUrl, token)} /> : <Pending />}
                  <Hint>
                    Any client that speaks Streamable HTTP: URL <Mono>{mcpUrl}</Mono>, header{" "}
                    <Mono>Authorization: Bearer &lt;token&gt;</Mono>. The field names differ a little per client.
                  </Hint>
                </Step>
              </>
            )}
          </ol>
        )}

        {client !== "chatgpt" && local && (
          <Hint>
            <Mono>{host}</Mono> only works for agents on this {host === "localhost" || host.startsWith("127.") ? "computer" : "network"}.
            For agents elsewhere, give Eunomia a public address under{" "}
            <Link href="/settings?tab=https" className="underline" style={{ color: "var(--accent-text)" }}>
              Settings, HTTPS
            </Link>
            .
          </Hint>
        )}

        {error && <ErrorLine error={error}>{error.message} Try again, or create one under Settings, API tokens.</ErrorLine>}
      </div>

      <div className="flex flex-col gap-1.5 pt-3.5" style={{ borderTop: "var(--hair) solid var(--border)" }}>
        <Hint>
          Or connect every agent on this machine at once (Claude Code, Codex, Cursor, Gemini CLI, …) from the install
          folder.{" "}
          <Link href="/docs" className="underline" style={{ color: "var(--accent-text)" }}>
            Read the docs
          </Link>
        </Hint>
        <Code label="Script" value="./scripts/connect-agents.sh --email you@… --password …" />
      </div>
    </div>
  );
}

function Pending() {
  return (
    <div className="field px-2.5 py-2 text-[12px]" style={{ color: "var(--ink-faint)" }}>
      Generate the token first and it appears here, filled in.
    </div>
  );
}

function ChatGptGuide({ mcpUrl, reachable }: { mcpUrl: string; reachable: boolean }) {
  return (
    <>
      {!reachable && (
        <div className="rounded-[7px] px-3 py-2 text-[12.5px] leading-relaxed" style={{ background: "var(--surface-raised)", color: "var(--ink-dim)" }}>
          ChatGPT calls Eunomia from OpenAI&apos;s servers, so it needs a public <Mono>https://</Mono> address, not{" "}
          <Mono>{mcpUrl.replace(/\/mcp$/, "")}</Mono>. Set one up under{" "}
          <Link href="/settings?tab=https" className="underline" style={{ color: "var(--accent-text)" }}>
            Settings, HTTPS
          </Link>{" "}
          (or a tunnel), then open this page from that address.
        </div>
      )}
      <ol className="flex flex-col gap-4">
        <Step n={1} title="Copy the server URL">
          <div className="field flex items-center gap-2 h-8 pl-2.5 pr-1">
            <code className="flex-1 min-w-0 truncate text-[12px] font-mono" style={{ color: "var(--ink)" }}>
              {mcpUrl}
            </code>
            <CopyButton value={mcpUrl} label="Copy URL" className="shrink-0" />
          </div>
        </Step>
        <Step n={2} title="Add it as a custom MCP server">
          <Hint>
            In ChatGPT on the web, open <Mono>chatgpt.com/plugins</Mono>, choose +, then Add custom MCP server. Name it
            Eunomia, paste the URL, set authentication to OAuth, continue past the warning and choose Create as a
            plugin. ChatGPT sends you to Eunomia to approve access, so there is no token to paste.
          </Hint>
          <Hint>
            Custom MCP servers depend on your plan and workspace: Business, Enterprise and Edu get full access, Pro
            read-only tools (it can recall but not save).
          </Hint>
        </Step>
        <Step n={3} title="Check it">
          <Hint>
            Install the plugin from your personal plugins. In a chat, type <Mono>@</Mono>, pick Eunomia and ask
            &ldquo;What do you remember about me?&rdquo;
          </Hint>
        </Step>
      </ol>
    </>
  );
}
