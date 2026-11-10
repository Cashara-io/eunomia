"use client";

import { useState } from "react";
import { EyeIcon, EyeSlashIcon, KeyIcon, ShieldCheckIcon, XIcon } from "@phosphor-icons/react/ssr";
import type { RequestOut } from "@/lib/gen";
import { useDeclineSecretRequest, useFillSecretRequest } from "@/lib/queries/secretRequests";
import { useVaults } from "@/lib/queries/vaults";
import { relativeTime } from "@/lib/connectorMeta";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";

const PLACEMENTS = [
  { value: "header", label: "Headers" },
  { value: "query", label: "Query string" },
  { value: "body", label: "Request body" },
] as const;

const CHANNEL: Record<string, string> = { mcp: "MCP", rest: "API", chat: "web chat" };

/** Who asked, as the person can recognise it: the token's or app's name, the channel, the session. */
export function askedBy(r: RequestOut): string {
  const who = r.client_name || (r.actor_kind === "token" ? "a personal access token" : r.actor_kind === "oauth" ? "a connected app" : "you, in Eunomia");
  const via = CHANNEL[r.channel] ?? r.channel;
  return via ? `${who} (${via})` : who;
}

/**
 * The secret request pop-up's body (docs/secret-requests.md), shared by the modal and `/secrets/requests/<id>`.
 * The value lives only in this component's state and goes to one place: the fill route. It is never shown
 * again, never logged, and the agent that asked only ever gets the handle.
 */
export default function SecretRequestForm({ request, onDone, onLater }: { request: RequestOut; onDone?: (outcome: "filled" | "declined") => void; onLater?: () => void }) {
  const fill = useFillSecretRequest();
  const decline = useDeclineSecretRequest();
  const vaults = useVaults().data ?? [];
  const [value, setValue] = useState("");
  const [show, setShow] = useState(false);
  const [hosts, setHosts] = useState<string[]>(request.target_hosts);
  const [newHost, setNewHost] = useState("");
  const [placements, setPlacements] = useState<string[]>(request.placements);
  const [anyAgent, setAnyAgent] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const vaultName = vaults.find((v) => v.id === request.vault_id)?.name ?? request.vault_id;
  const id = `secret-${request.id.replace(/[^a-z0-9]/gi, "-")}`;

  async function save() {
    setError(null);
    try {
      await fill.mutateAsync({ id: request.id, body: { value, allowed_hosts: hosts, placements, any_agent: anyAgent } });
      setValue("");
      onDone?.("filled");
    } catch (e) {
      setError(failure(e, "Could not save the secret. Check it and try again."));
    }
  }

  async function refuse() {
    setError(null);
    try {
      await decline.mutateAsync(request.id);
      setValue("");
      onDone?.("declined");
    } catch (e) {
      setError(failure(e, "Could not decline the request. Try again."));
    }
  }

  function addHost() {
    const h = newHost.trim().toLowerCase();
    if (h && !hosts.includes(h)) setHosts([...hosts, h]);
    setNewHost("");
  }

  const closed = request.status !== "pending";
  return (
    <form
      className="flex flex-col gap-4"
      data-testid="secret-request-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (request.can_fill && value.trim()) save();
      }}
    >
      <div className="flex items-start gap-3">
        <KeyIcon weight="duotone" size={18} aria-hidden style={{ color: "var(--accent-text)", marginTop: 2 }} />
        <div className="min-w-0 flex-1">
          <div className="text-reading font-medium" data-testid="secret-request-label">
            {request.label || request.name}
          </div>
          <div className="text-detail mt-0.5" style={{ color: "var(--ink-dim)" }}>
            Asked by {askedBy(request)} {relativeTime(request.created_at)}
            {request.session ? <span style={{ color: "var(--ink-faint)" }}> · session {request.session.slice(0, 12)}</span> : null}
          </div>
        </div>
      </div>

      {request.description && (
        <div className="text-detail">
          <div className="label mb-1">Why, in the agent&apos;s words</div>
          <p className="whitespace-pre-wrap break-words" style={{ color: "var(--ink-dim)" }}>
            {request.description}
          </p>
        </div>
      )}

      <dl className="grid grid-cols-[110px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-detail">
        <dt style={{ color: "var(--ink-faint)" }}>Kept in</dt>
        <dd>{vaultName}</dd>
        <dt style={{ color: "var(--ink-faint)" }}>Agents get</dt>
        <dd className="font-mono break-all">{request.handle}</dd>
        {request.connector && (
          <>
            <dt style={{ color: "var(--ink-faint)" }}>For</dt>
            <dd>{request.connector}</dd>
          </>
        )}
        <dt style={{ color: "var(--ink-faint)" }}>Expires</dt>
        <dd>{new Date(request.expires_at).toLocaleString()}</dd>
      </dl>

      {closed ? (
        <p className="ledger px-4 py-3 text-detail" role="status" style={{ color: "var(--ink-dim)" }}>
          This request is {request.status}.
        </p>
      ) : !request.can_fill ? (
        <p className="ledger px-4 py-3 text-detail" role="status" style={{ color: "var(--ink-dim)" }}>
          Only the person whose agent asked can paste this secret. You can decline it.
        </p>
      ) : (
        <>
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${id}-value`} className="label">
              Secret
            </label>
            <div className="flex items-center gap-2">
              <input
                id={`${id}-value`}
                data-testid="secret-value"
                type={show ? "text" : "password"}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                autoFocus
                className="field h-9 px-3 text-detail font-mono w-full"
                value={value}
                onChange={(e) => setValue(e.target.value)}
              />
              <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label={show ? "Hide the secret" : "Show the secret"} aria-pressed={show} onClick={() => setShow(!show)}>
                {show ? <EyeSlashIcon weight="duotone" size={15} /> : <EyeIcon weight="duotone" size={15} />}
              </button>
            </div>
            <span className="text-caption flex items-center gap-1.5" style={{ color: "var(--ink-faint)" }}>
              <ShieldCheckIcon weight="duotone" size={13} aria-hidden />
              Encrypted before it is stored. Nobody can read it back, you included; the agent only gets the handle.
            </span>
          </div>

          <fieldset className="flex flex-col gap-1.5">
            <legend className="label mb-1">May be sent to</legend>
            {hosts.length === 0 && (
              <span className="text-detail" style={{ color: "var(--warning)" }}>
                No host: agents could only transform it, not send it anywhere.
              </span>
            )}
            <ul className="flex flex-wrap gap-1.5">
              {hosts.map((h) => (
                <li key={h} className="inline-flex items-center gap-1 rounded px-2 h-7 text-detail font-mono" style={{ background: "var(--surface-raised)" }}>
                  {h}
                  <button type="button" className="btn btn-ghost btn-icon" style={{ height: 20, width: 20 }} aria-label={`Remove ${h}`} onClick={() => setHosts(hosts.filter((x) => x !== h))}>
                    <XIcon weight="duotone" size={12} />
                  </button>
                </li>
              ))}
            </ul>
            <div className="flex items-center gap-2">
              <input
                aria-label="Add a host"
                className="field h-8 px-3 text-detail font-mono flex-1"
                placeholder="api.example.com"
                value={newHost}
                onChange={(e) => setNewHost(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    addHost();
                  }
                }}
              />
              <button type="button" className="btn btn-sm" onClick={addHost} disabled={!newHost.trim()}>
                Add
              </button>
            </div>
          </fieldset>

          <fieldset className="flex flex-col gap-1">
            <legend className="label mb-1">Where in a request</legend>
            <div className="flex flex-wrap gap-4 text-detail">
              {PLACEMENTS.map((p) => (
                <label key={p.value} className="inline-flex items-center gap-1.5">
                  <input
                    type="checkbox"
                    checked={placements.includes(p.value)}
                    onChange={(e) => setPlacements(e.target.checked ? [...placements, p.value] : placements.filter((x) => x !== p.value))}
                  />
                  {p.label}
                </label>
              ))}
            </div>
          </fieldset>

          <label className="inline-flex items-start gap-2 text-detail">
            <input type="checkbox" className="mt-0.5" checked={anyAgent} onChange={(e) => setAnyAgent(e.target.checked)} />
            <span>
              Let any of my agents in {vaultName} use it
              <span className="block text-caption" style={{ color: "var(--ink-faint)" }}>
                Otherwise only {askedBy(request)} may.
              </span>
            </span>
          </label>
        </>
      )}

      <p className="text-caption" style={{ color: "var(--ink-faint)" }}>
        Eunomia never asks for a secret in chat or on another site. Paste it only on this page.
      </p>

      {error && <ErrorLine error={error} />}

      <div className="flex items-center gap-2">
        {request.can_fill && !closed && (
          <button type="submit" className="btn btn-primary btn-sm" disabled={fill.isPending || !value.trim()} aria-busy={fill.isPending}>
            {fill.isPending ? "Saving…" : "Save"}
          </button>
        )}
        {request.can_decline && !closed && (
          <button type="button" className="btn btn-sm" onClick={refuse} disabled={decline.isPending}>
            Decline
          </button>
        )}
        {onLater && (
          <button type="button" className="btn btn-ghost btn-sm" onClick={onLater}>
            Later
          </button>
        )}
      </div>
    </form>
  );
}
