"use client";

import { useState } from "react";
import { KeyRound, Pencil, Plus, PlugZap, Trash2, X } from "lucide-react";
import type { ProviderOut, RefCount, RefOut } from "@/lib/gen";
import { ApiError } from "@/lib/api";
import {
  useCreateSecretRef,
  useDeleteSecretProvider,
  useDeleteSecretRef,
  usePutSecretProvider,
  useSecretCounts,
  useSecretLog,
  useSecretProviders,
  useSecretRefs,
  useTestSecretProvider,
  useUpdateSecretRef,
} from "@/lib/queries/secrets";
import { useVaults } from "@/lib/queries/vaults";
import { relativeTime } from "@/lib/connectorMeta";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import ConfirmButton from "@/components/ConfirmButton";
import Select from "@/components/Select";
import { ICON } from "../settings/shared";

/** The kinds the broker supports, in the order shown (docs/password-managers.md). */
const KINDS = [
  { kind: "onepassword_connect", name: "1Password Connect", host: "required", hostLabel: "Connect server URL", hostHint: "https://connect.example.com:8080", scheme: "op" },
  { kind: "onepassword_service_account", name: "1Password service account", host: "none", hostLabel: "", hostHint: "", scheme: "op" },
  { kind: "bitwarden_sm", name: "Bitwarden Secrets Manager", host: "optional", hostLabel: "Server URL (optional)", hostHint: "https://vault.bitwarden.eu, or empty for the US cloud", scheme: "bws" },
] as const;
type Kind = (typeof KINDS)[number]["kind"];
const kindName = (k: string) => KINDS.find((x) => x.kind === k)?.name ?? k;

const OUTCOME_COLOUR: Record<string, string> = { ok: "var(--good)", denied: "var(--critical)", not_found: "var(--warning)", provider_error: "var(--critical)" };
const OUTCOME_LABEL: Record<string, string> = { ok: "Resolved", denied: "Denied", not_found: "Not found", provider_error: "Provider error" };

const list = (s: string) =>
  s
    .split(/[,\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);

export default function PasswordManagersSection() {
  const providers = useSecretProviders();
  const forbidden = providers.error instanceof ApiError && providers.error.status === 403;
  return (
    <section className="flex flex-col gap-3" aria-labelledby="password-managers-heading">
      <div className="min-w-0">
        <h2 id="password-managers-heading" className="section-title">
          Password managers
        </h2>
        <p className="text-[12.5px] mt-1 max-w-[66ch]" style={{ color: "var(--ink-dim)" }}>
          Let tools use secrets kept in your own 1Password or Bitwarden Secrets Manager. Eunomia stores only references such as <code className="font-mono">op://Ops/Square/access_token</code>, resolves them for one tool call at a
          time, and logs every use. Values are never stored.
        </p>
      </div>
      {forbidden ? (
        <p className="ledger px-4 py-4 text-[13px]" style={{ color: "var(--ink-dim)" }}>
          Only an owner of this organisation or an instance admin can manage password managers.
        </p>
      ) : (
        <>
          {providers.isError && <ErrorLine error={failure(providers.error, "Could not load the password managers.", " Reload the page to try again.")} />}
          <Providers rows={providers.data ?? null} />
          <References />
          <ResolutionLog />
        </>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- providers

function Providers({ rows }: { rows: ProviderOut[] | null }) {
  return (
    <div className="ledger overflow-hidden">
      <div className="hairline-rows">
        {KINDS.map((k) => (
          <ProviderRow key={k.kind} spec={k} row={rows?.find((r) => r.kind === k.kind) ?? null} loading={rows === null} />
        ))}
      </div>
    </div>
  );
}

function ProviderRow({ spec, row, loading }: { spec: (typeof KINDS)[number]; row: ProviderOut | null; loading: boolean }) {
  const put = usePutSecretProvider();
  const remove = useDeleteSecretProvider();
  const test = useTestSecretProvider();
  const [editing, setEditing] = useState(false);
  const [host, setHost] = useState("");
  const [token, setToken] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const configured = !!row;
  const id = `pm-${spec.kind}`;

  function open() {
    setHost(row?.host ?? "");
    setToken("");
    setError(null);
    setResult(null);
    setEditing(true);
  }

  async function save() {
    setError(null);
    try {
      await put.mutateAsync({ kind: spec.kind, body: { host: spec.host === "none" ? null : host.trim() || null, token: token.trim() || null } });
      setToken("");
      setEditing(false);
      setResult({ ok: true, text: "Saved. Test the connection to check the token." });
    } catch (e) {
      setError(failure(e, "Could not save the password manager. Check the URL and token and try again."));
    }
  }

  async function runTest() {
    setError(null);
    setResult(null);
    try {
      const out = await test.mutateAsync(spec.kind);
      setResult(out.ok ? { ok: true, text: `Connected: ${out.detail ?? "ok"}.` } : { ok: false, text: out.message || `The test failed (${out.error ?? "error"}).` });
    } catch (e) {
      setError(failure(e, "Could not run the test. Try again."));
    }
  }

  return (
    <div className="px-4 py-3 flex flex-col gap-3" data-testid={id}>
      <div className="flex flex-wrap items-center gap-3">
        <KeyRound {...ICON} aria-hidden style={{ color: "var(--ink-faint)" }} />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium">{spec.name}</div>
          <div className="text-[12px] flex items-center gap-1.5" style={{ color: "var(--ink-dim)" }}>
            {loading ? (
              <span className="skeleton h-3 w-24" aria-hidden />
            ) : configured ? (
              <>
                <span className="dot" style={{ background: "var(--good)" }} aria-hidden />
                <span>Configured{row?.host ? ` · ${row.host}` : ""}</span>
                <span style={{ color: "var(--ink-faint)" }}>updated {relativeTime(row?.updated_at)}</span>
              </>
            ) : (
              <span>Not set up</span>
            )}
          </div>
        </div>
        {!editing && (
          <div className="flex items-center gap-2">
            {configured && (
              <button type="button" className="btn btn-sm" onClick={runTest} disabled={test.isPending} aria-busy={test.isPending}>
                <PlugZap {...ICON} aria-hidden />
                {test.isPending ? "Testing…" : "Test connection"}
              </button>
            )}
            <button type="button" className="btn btn-sm" onClick={open}>
              {configured ? "Replace" : "Set up"}
            </button>
            {configured && (
              <ConfirmButton
                label={`Remove ${spec.name}`}
                confirmLabel="Remove"
                className="btn btn-ghost btn-sm btn-icon"
                disabled={remove.isPending}
                onConfirm={() => remove.mutateAsync(spec.kind).catch((e) => setError(failure(e, "Could not remove it. Try again.")))}
              >
                <Trash2 {...ICON} />
              </ConfirmButton>
            )}
          </div>
        )}
      </div>

      {editing && (
        <form
          className="flex flex-col gap-3 pop-in"
          style={{ transformOrigin: "top center" }}
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          {spec.host !== "none" && (
            <div className="flex flex-col gap-1.5">
              <label htmlFor={`${id}-host`} className="label">
                {spec.hostLabel}
              </label>
              <input
                id={`${id}-host`}
                className="field h-8 px-3 text-[13px] font-mono w-full"
                placeholder={spec.hostHint}
                required={spec.host === "required"}
                value={host}
                onChange={(e) => setHost(e.target.value)}
              />
            </div>
          )}
          <div className="flex flex-col gap-1.5">
            <label htmlFor={`${id}-token`} className="label">
              {configured ? "New token (leave empty to keep the stored one)" : "Token"}
            </label>
            <input
              id={`${id}-token`}
              type="password"
              autoComplete="off"
              spellCheck={false}
              className="field h-8 px-3 text-[13px] font-mono w-full"
              required={!configured}
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <span className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
              Write-only: it is encrypted before it is stored and is never shown again.
            </span>
          </div>
          <div className="flex items-center gap-2">
            <button type="submit" className="btn btn-primary btn-sm" disabled={put.isPending} aria-busy={put.isPending}>
              {put.isPending ? "Saving…" : "Save"}
            </button>
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </div>
        </form>
      )}
      {result &&
        (result.ok ? (
          <p role="status" className="fade-in text-[12.5px]" style={{ color: "var(--good)" }}>
            {result.text}
          </p>
        ) : (
          <ErrorLine error={{ message: result.text }} />
        ))}
      {error && <ErrorLine error={error} />}
    </div>
  );
}

// ---------------------------------------------------------------- references

type Draft = { reference: string; provider: string; vault_id: string; label: string; tools: string; actors: string };
const emptyDraft: Draft = { reference: "", provider: "", vault_id: "", label: "", tools: "", actors: "" };

function guessProvider(ref: string, configured: string[]): string {
  const scheme = ref.trim().toLowerCase().startsWith("bws://") ? "bws" : "op";
  const fits = KINDS.filter((k) => k.scheme === scheme).map((k) => k.kind as string);
  return fits.find((k) => configured.includes(k)) ?? fits[0] ?? "";
}

function References() {
  const refs = useSecretRefs();
  const counts = useSecretCounts();
  const providers = useSecretProviders();
  const vaults = useVaults().data ?? [];
  const create = useCreateSecretRef();
  const update = useUpdateSecretRef();
  const remove = useDeleteSecretRef();
  const [draft, setDraft] = useState<Draft | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const rows = refs.data ?? null;
  const configured = (providers.data ?? []).map((p) => p.kind);
  const vaultName = (id: string) => vaults.find((v) => v.id === id)?.name ?? id;
  const countOf = (r: RefOut): RefCount | undefined => (counts.data ?? []).find((c) => c.ref_id === r.id || c.reference === r.reference);

  function startNew() {
    setEditingId(null);
    setError(null);
    setDraft({ ...emptyDraft, vault_id: vaults.find((v) => v.kind === "personal")?.id ?? vaults[0]?.id ?? "" });
  }

  function startEdit(r: RefOut) {
    setEditingId(r.id);
    setError(null);
    setDraft({ reference: r.reference, provider: r.provider, vault_id: r.vault_id, label: r.label === r.reference ? "" : r.label, tools: r.allowed_tools.join(", "), actors: r.allowed_actors.join(", ") });
  }

  async function save() {
    if (!draft) return;
    setError(null);
    const body = {
      reference: draft.reference.trim(),
      provider: draft.provider || guessProvider(draft.reference, configured),
      vault_id: draft.vault_id,
      label: draft.label.trim() || null,
      allowed_tools: list(draft.tools),
      allowed_actors: list(draft.actors),
    };
    try {
      if (editingId) await update.mutateAsync({ id: editingId, body });
      else await create.mutateAsync(body);
      setDraft(null);
      setEditingId(null);
    } catch (e) {
      setError(failure(e, "Could not save the reference. Check it and try again."));
    }
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-3 mt-2">
        <h3 className="text-[13px] font-medium">References</h3>
        {!draft && (
          <button type="button" className="btn btn-sm" onClick={startNew}>
            <Plus {...ICON} aria-hidden />
            Add reference
          </button>
        )}
      </div>

      {draft && (
        <form
          className="panel pop-in p-4 flex flex-col gap-3"
          style={{ transformOrigin: "top center" }}
          aria-label={editingId ? "Edit reference" : "New reference"}
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <div className="flex items-start justify-between gap-3">
            <div className="text-[13px] font-medium">{editingId ? "Edit reference" : "New reference"}</div>
            <button type="button" onClick={() => setDraft(null)} aria-label="Cancel" className="btn btn-ghost btn-sm btn-icon -mt-1 -mr-1">
              <X {...ICON} />
            </button>
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="ref-reference" className="label">
              Reference
            </label>
            <input
              id="ref-reference"
              required
              disabled={!!editingId}
              className="field h-8 px-3 text-[13px] font-mono w-full"
              placeholder="op://Ops/Square/access_token or bws://<secret id>"
              value={draft.reference}
              onChange={(e) => setDraft({ ...draft, reference: e.target.value, provider: guessProvider(e.target.value, configured) })}
            />
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <span className="label">Resolved by</span>
              <Select
                aria-label="Password manager"
                value={draft.provider || guessProvider(draft.reference, configured)}
                onChange={(provider) => setDraft({ ...draft, provider })}
                disabled={!!editingId}
                options={KINDS.map((k) => ({ value: k.kind, label: k.name, hint: configured.includes(k.kind) ? "configured" : undefined }))}
                className="h-8 text-[13px] w-full"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <span className="label">Bound to vault</span>
              <Select aria-label="Vault" value={draft.vault_id} onChange={(vault_id) => setDraft({ ...draft, vault_id })} options={vaults.map((v) => ({ value: v.id, label: v.name }))} className="h-8 text-[13px] w-full" />
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="ref-tools" className="label">
              Allowed tools (comma separated; * is any tool that takes secrets)
            </label>
            <input id="ref-tools" className="field h-8 px-3 text-[13px] font-mono w-full" placeholder="square_charge" value={draft.tools} onChange={(e) => setDraft({ ...draft, tools: e.target.value })} />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="ref-actors" className="label">
              Allowed people or tokens (optional; empty is any member of the vault)
            </label>
            <input id="ref-actors" className="field h-8 px-3 text-[13px] font-mono w-full" placeholder="user:… or api_token:…" value={draft.actors} onChange={(e) => setDraft({ ...draft, actors: e.target.value })} />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="ref-label" className="label">
              Label (optional)
            </label>
            <input id="ref-label" className="field h-8 px-3 text-[13px] w-full" maxLength={120} value={draft.label} onChange={(e) => setDraft({ ...draft, label: e.target.value })} />
          </div>
          <div>
            <button type="submit" className="btn btn-primary btn-sm" disabled={create.isPending || update.isPending || !draft.reference.trim() || !draft.vault_id}>
              {create.isPending || update.isPending ? "Saving…" : editingId ? "Save reference" : "Add reference"}
            </button>
          </div>
        </form>
      )}
      {error && <ErrorLine error={error} />}
      {refs.isError && <ErrorLine error={failure(refs.error, "Could not load the references.", " Reload the page to try again.")} />}

      <div className="ledger overflow-hidden">
        <table className="w-full text-[13px]">
          <thead>
            <tr className="label border-b text-left h-8">
              <th className="px-4 font-medium">Reference</th>
              <th className="px-3 font-medium hidden md:table-cell">Vault</th>
              <th className="px-3 font-medium hidden md:table-cell">Tools</th>
              <th className="px-3 font-medium text-right">Uses (7 days)</th>
              <th className="px-3">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody className="hairline-rows">
            {rows === null && !refs.isError && (
              <tr aria-hidden>
                <td colSpan={5} className="px-4 h-10">
                  <span className="skeleton h-3 w-48" />
                </td>
              </tr>
            )}
            {rows?.length === 0 && (
              <tr>
                <td colSpan={5} className="px-4 h-14" style={{ color: "var(--ink-dim)" }}>
                  No references yet. Add one to let a tool use a secret from your password manager.
                </td>
              </tr>
            )}
            {rows?.map((r) => {
              const c = countOf(r);
              const refused = (c?.denied ?? 0) + (c?.not_found ?? 0) + (c?.provider_error ?? 0);
              return (
                <tr key={r.id} className="h-10" data-testid="secret-ref-row">
                  <td className="px-4 min-w-0">
                    <div className="font-mono text-[12.5px] truncate">{r.reference}</div>
                    <div className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
                      {r.label !== r.reference ? `${r.label} · ` : ""}
                      {kindName(r.provider)}
                    </div>
                  </td>
                  <td className="px-3 hidden md:table-cell">{vaultName(r.vault_id)}</td>
                  <td className="px-3 hidden md:table-cell font-mono text-[12px]">{r.allowed_tools.length ? r.allowed_tools.join(", ") : <span style={{ color: "var(--ink-faint)" }}>none</span>}</td>
                  <td className="px-3 text-right tabular-nums">
                    {c?.ok ?? 0}
                    {refused > 0 && <span style={{ color: "var(--critical)" }}> · {refused} refused</span>}
                  </td>
                  <td className="px-3">
                    <div className="flex items-center justify-end gap-1">
                      <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label={`Edit ${r.reference}`} onClick={() => startEdit(r)}>
                        <Pencil {...ICON} />
                      </button>
                      <ConfirmButton
                        label={`Remove ${r.reference}`}
                        confirmLabel="Remove"
                        className="btn btn-ghost btn-sm btn-icon"
                        disabled={remove.isPending}
                        onConfirm={() => remove.mutateAsync(r.id).catch((e) => setError(failure(e, "Could not remove the reference. Try again.")))}
                      >
                        <Trash2 {...ICON} />
                      </ConfirmButton>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- resolution log

function ResolutionLog() {
  const log = useSecretLog();
  const entries = log.data?.entries ?? null;
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-[13px] font-medium mt-2">Resolution log</h3>
      <p className="text-[12.5px] -mt-1" style={{ color: "var(--ink-dim)" }}>
        Every time a tool asked for a secret: who, which reference, which tool, and the outcome. Never the value. Tool calls themselves are under Settings, Usage.
      </p>
      {log.isError && <ErrorLine error={failure(log.error, "Could not load the resolution log.", " Reload the page to try again.")} />}
      <div className="ledger overflow-hidden">
        <table className="w-full text-[13px]">
          <thead>
            <tr className="label border-b text-left h-8">
              <th className="px-4 font-medium">When</th>
              <th className="px-3 font-medium">Reference</th>
              <th className="px-3 font-medium hidden md:table-cell">Tool</th>
              <th className="px-3 font-medium hidden lg:table-cell">Who</th>
              <th className="px-3 font-medium">Outcome</th>
            </tr>
          </thead>
          <tbody className="hairline-rows">
            {entries === null && !log.isError && (
              <tr aria-hidden>
                <td colSpan={5} className="px-4 h-10">
                  <span className="skeleton h-3 w-40" />
                </td>
              </tr>
            )}
            {entries?.length === 0 && (
              <tr>
                <td colSpan={5} className="px-4 h-14" style={{ color: "var(--ink-dim)" }}>
                  No secret has been resolved yet.
                </td>
              </tr>
            )}
            {entries?.map((e) => (
              <tr key={e.id} className="h-10" data-testid="secret-log-row">
                <td className="px-4 whitespace-nowrap" style={{ color: "var(--ink-dim)" }}>
                  {relativeTime(e.at)}
                </td>
                <td className="px-3 font-mono text-[12.5px] truncate max-w-[280px]">{e.reference}</td>
                <td className="px-3 hidden md:table-cell font-mono text-[12px]">{e.tool}</td>
                <td className="px-3 hidden lg:table-cell font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {e.actor_id} · {e.channel}
                </td>
                <td className="px-3">
                  <span className="inline-flex items-center gap-1.5" title={e.reason || undefined}>
                    <span className="dot" style={{ background: OUTCOME_COLOUR[e.outcome] ?? "var(--ink-faint)" }} aria-hidden />
                    {OUTCOME_LABEL[e.outcome] ?? e.outcome}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export type { Kind };
