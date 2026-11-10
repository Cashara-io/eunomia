"use client";

// The API-token way to connect an AI app: one tab per app, a token minted for
// that app, and the exact command or config with this instance's URL and the
// token filled in. Shown as the advanced option under onboarding and the
// /connect guides, which lead with sign-in instead. ChatGPT has no tab: it can
// only sign in, never use a pasted token.

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useRef, useState } from "react";
import Link from "next/link";
import { KeyIcon } from "@phosphor-icons/react/ssr";
import { useVaults } from "@/lib/queries/vaults";
import { useCreateToken } from "@/lib/queries/auth";
import CopyButton from "@/components/bits/CopyButton";
import DecryptReveal from "@/components/bits/DecryptReveal";
import { isPrivateHost, useMcpAddress } from "@/components/connect/Guide";

const CLIENTS = [
  { id: "claude", label: "Claude Code", token: "Claude Code" },
  { id: "codex", label: "Codex", token: "Codex" },
  { id: "other", label: "Other", token: "MCP" },
] as const;

type ClientId = (typeof CLIENTS)[number]["id"];

const claudeCodeCommand = (url: string, token: string) =>
  `claude mcp add --transport http --scope user eunomia ${url} --header "Authorization: Bearer ${token}"`;

const codexConfig = (url: string, token: string) =>
  `[mcp_servers.eunomia]\nurl = "${url}"\nhttp_headers = { Authorization = "Bearer ${token}" }`;

const otherConfig = (url: string, token: string) =>
  JSON.stringify({ mcpServers: { eunomia: { type: "http", url, headers: { Authorization: `Bearer ${token}` } } } }, null, 2);

function Code({ value, label }: { value: string; label: string }) {
  return (
    <div className="flex flex-col gap-2 items-start">
      <pre
        aria-label={label}
        className="w-full text-[13px] font-mono whitespace-pre-wrap [overflow-wrap:anywhere] px-4 py-3 rounded-[12px]"
        style={{ background: "var(--surface-raised)", color: "var(--ink)", border: "1px solid var(--border)" }}
      >
        {value}
      </pre>
      <CopyButton value={value} label={`Copy ${label.toLowerCase()}`} size="sm" />
    </div>
  );
}

function Step({ n, title, children }: { n: number; title: React.ReactNode; children?: React.ReactNode }) {
  return (
    <li className="grid grid-cols-[22px_minmax(0,1fr)] gap-x-2 gap-y-2">
      <span className="text-[15px] font-semibold leading-[22px]" aria-hidden>
        {n}.
      </span>
      <div className="text-[15px] font-semibold leading-[22px]">{title}</div>
      {children && <div className="col-start-2 flex flex-col gap-1.5 min-w-0">{children}</div>}
    </li>
  );
}

const Mono = ({ children }: { children: React.ReactNode }) => <code className="font-mono text-[0.92em]">{children}</code>;
const Hint = ({ children }: { children: React.ReactNode }) => (
  <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
    {children}
  </p>
);

/** `onToken` fires after a token is minted, so a wizard can turn Skip into Continue.
 * Without `initialClient` no app is preselected until one is chosen. */
export default function ConnectAgents({ onToken, initialClient }: { onToken?: () => void; initialClient?: ClientId }) {
  const mcpUrl = useMcpAddress();
  const [client, setClient] = useState<ClientId | null>(initialClient ?? null);
  // One token per client, shown once; kept while the guide stays mounted.
  const [tokens, setTokens] = useState<Partial<Record<ClientId, string>>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const createToken = useCreateToken();
  const vaultsQuery = useVaults();
  const personal = vaultsQuery.isSuccess ? vaultsQuery.data.find((v) => v.is_default && v.id) : undefined;
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);

  const current = CLIENTS.find((c) => c.id === client) ?? null;
  const token = client ? tokens[client] : undefined;
  const host = mcpUrl ? new URL(mcpUrl).hostname : "";
  const local = isPrivateHost(host);

  async function mint() {
    if (!current || !client || !personal) return;
    setBusy(true);
    setError(null);
    try {
      const res = await createToken.mutateAsync({
        name: current.token,
        scopes: ["memory:read", "memory:write"],
        vault_id: personal.id,
        expires_at: new Date(Date.now() + 90 * 86_400_000).toISOString(),
      });
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

  const mintStep = current && (
    <Step n={1} title={token ? "Your token, shown once" : "Generate a token for " + current.label}>
      {token ? (
        <div className="field flex items-center gap-2 min-h-10 pl-3 pr-1">
          <code className="flex-1 min-w-0 truncate text-[13px] font-mono" style={{ color: "var(--ink)" }}>
            <DecryptReveal key={token} text={token} />
          </code>
          <CopyButton value={token} label="Copy token" className="shrink-0" />
        </div>
      ) : (
        <button onClick={mint} disabled={busy || !personal} aria-busy={busy} className="btn btn-primary self-start">
          <KeyIcon weight="duotone" size={14} aria-hidden />
          {busy ? "Creating…" : "Generate a token"}
        </button>
      )}
      {!token && !personal && (
        <Hint>{vaultsQuery.isPending ? "Loading your Personal memory space…" : "Your Personal memory space could not be loaded. Reload to try again."}</Hint>
      )}
      <Hint>Reads and saves memories in Personal. Expires in 90 days. Choose other permissions or a lifetime in Settings, API tokens.</Hint>
      <Hint>
        Named &ldquo;{current.token}&rdquo; under{" "}
        <Link href="/settings?tab=tokens" className="link">
          Settings, API tokens
        </Link>
        , so you can revoke it on its own.
      </Hint>
    </Step>
  );

  return (
    <div className="flex flex-col gap-5">
      <div role="tablist" aria-label="AI app" className="tabs">
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
            tabIndex={client === c.id || (client === null && i === 0) ? 0 : -1}
            onClick={() => choose(c.id)}
            onKeyDown={(e) => onKey(e, i)}
            className="tab"
          >
            {c.label}
          </button>
        ))}
      </div>

      {client === null ? (
        <Hint>Choose your app above to see its token steps.</Hint>
      ) : (
      <div role="tabpanel" id="connect-panel" aria-labelledby={`connect-tab-${client}`} className="flex flex-col gap-4 min-w-0">
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
                <Step n={2} title="Add it to your app's settings">
                  {token ? <Code label="Config" value={otherConfig(mcpUrl, token)} /> : <Pending />}
                  <Hint>
                    Most apps that can add a server by web address work: URL <Mono>{mcpUrl}</Mono>, header{" "}
                    <Mono>Authorization: Bearer &lt;token&gt;</Mono>. The field names differ a little per app.
                  </Hint>
                </Step>
              </>
            )}
          </ol>

        {local && (
          <Hint>
            <Mono>{host}</Mono> only works for agents on this {host === "localhost" || host.startsWith("127.") ? "computer" : "network"}.
            For agents elsewhere, give Eunomia a public address under{" "}
            <Link href="/settings?tab=https" className="link">
              Settings, HTTPS
            </Link>
            .
          </Hint>
        )}

        {error && <ErrorLine error={error}>{error.message} Try again, or create one under Settings, API tokens.</ErrorLine>}
      </div>
      )}

      <div className="flex flex-col gap-1.5 pt-3.5" style={{ borderTop: "var(--hair) solid var(--border)" }}>
        <Hint>
          Or connect every agent on this machine at once (Claude Code, Codex, Cursor, Gemini CLI, …) from the install
          folder.{" "}
          <Link href="/docs" className="link">
            Read Help
          </Link>
        </Hint>
        <Code label="Script" value="./scripts/connect-agents.sh --email you@… --password …" />
      </div>
    </div>
  );
}

function Pending() {
  return (
    <div className="soft-panel px-4 py-3 text-[14px]" style={{ color: "var(--ink-dim)" }}>
      Generate the token first and the command appears here, filled in.
    </div>
  );
}
