"use client";

import { useState } from "react";
import { KeyRound, Pencil, Plus, PlugZap, Power, Trash2, X } from "lucide-react";
import type { HelperStatus, Kind as KindInfo, ProviderOut, RefCount, RefOut } from "@/lib/gen";
import {
  useCreateSecretRef,
  useDeleteSecretProvider,
  useDeleteSecretRef,
  usePutSecretProvider,
  useRequestSecretsHelper,
  useSecretAccess,
  useSecretCounts,
  useSecretKinds,
  useSecretProviders,
  useSecretRefs,
  useSecretsHelper,
  useTestSecretProvider,
  useUpdateSecretRef,
} from "@/lib/queries/secrets";
import { useVaults } from "@/lib/queries/vaults";
import { useSecretActivity } from "@/lib/queries/secretRequests";
import { relativeTime } from "@/lib/connectorMeta";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import ConfirmButton from "@/components/ConfirmButton";
import Select from "@/components/Select";
import { ICON } from "../settings/shared";

/**
 * How each kind is set up here (its form fields), in the order shown. Names, stability and the preview
 * note come from the API (`GET /api/secrets/kinds`, one source of truth in the backend); `name` is only
 * the fallback while that loads.
 */
const KINDS = [
  { kind: "onepassword_connect", name: "1Password Connect", host: "required", hostLabel: "Connect server URL", hostHint: "https://connect.example.com:8080", scheme: "op" },
  { kind: "onepassword_service_account", name: "1Password service account", host: "none", hostLabel: "", hostHint: "", scheme: "op" },
  { kind: "bitwarden_sm", name: "Bitwarden Secrets Manager", host: "optional", hostLabel: "Server URL (optional)", hostHint: "https://vault.bitwarden.eu, or empty for the US cloud", scheme: "bws" },
] as const;
type Kind = (typeof KINDS)[number]["kind"];

const OUTCOME_COLOUR: Record<string, string> = { ok: "var(--good)", denied: "var(--critical)", not_found: "var(--warning)", provider_error: "var(--critical)", blocked: "var(--critical)", timeout: "var(--warning)", connect: "var(--warning)" };
const OUTCOME_LABEL: Record<string, string> = { ok: "Resolved", denied: "Denied", not_found: "Not found", provider_error: "Provider error", blocked: "Blocked", timeout: "Timed out", connect: "Could not connect" };
/** The helper itself failed (`LogEntry.code`): clearer than "Provider error". */
const CODE_LABEL: Record<string, string> = { helper_disabled: "Helper off", helper_unreachable: "Helper not answering" };
/** The resolution log also shows secret requests and what agents did with held secrets (docs/secret-requests.md). */
const ACTION_LABEL: Record<string, string> = {
  "secret.resolve": "Used",
  "secret.request": "Requested",
  "secret.fill": "Filled",
  "secret.decline": "Declined",
  "secret.transform": "Transformed",
  "secret.http": "Sent",
  "secret.replace": "Replaced",
  "secret.delete": "Deleted",
};

const list = (s: string) =>
  s
    .split(/[,\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);

/** The API's metadata for a kind, and its display name. */
function useKindInfo() {
  const kinds = useSecretKinds().data ?? [];
  const info = (k: string): KindInfo | undefined => kinds.find((x) => x.kind === k);
  const name = (k: string) => info(k)?.label ?? KINDS.find((x) => x.kind === k)?.name ?? k;
  return { info, name };
}

/** The "Preview" badge for a kind the API marks as one (Bitwarden Secrets Manager). */
function PreviewBadge({ info }: { info: KindInfo | undefined }) {
  if (!info?.preview) return null;
  return (
    <span
      data-testid="preview-badge"
      className="inline-flex items-center rounded-[4px] px-1.5 text-[11px] font-medium leading-[18px]"
      style={{ color: "var(--warning)", border: "var(--hair) solid var(--warning)" }}
      title={info.note ?? undefined}
    >
      Preview
    </span>
  );
}

export default function PasswordManagersSection() {
  const access = useSecretAccess();
  const a = access.data;
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
      {access.isError && <ErrorLine error={failure(access.error, "Could not load the password managers.", " Reload the page to try again.")} />}
      {a && !a.manage_refs ? (
        <p className="ledger px-4 py-4 text-[13px]" style={{ color: "var(--ink-dim)" }}>
          Only an owner of this organisation, an instance admin or a vault owner can manage password managers.
        </p>
      ) : a ? (
        <>
          <Helper admin={a.toggle_helper} />
          {!a.manage_providers && (
            <p className="text-[12.5px] max-w-[66ch]" style={{ color: "var(--ink-dim)" }} data-testid="vault-owner-note">
              Only an owner of this organisation or an instance admin can set up a password manager. As a vault owner you can manage the references of the vaults you own and see their resolution log; their secret requests and held secrets are under Secrets agents asked for.
            </p>
          )}
          <Providers manage={a.manage_providers} />
          <References vaults={a.vaults} />
          <ResolutionLog />
        </>
      ) : (
        <div className="ledger p-5 flex flex-col gap-3" aria-hidden>
          <span className="skeleton h-4 w-48" />
          <span className="skeleton h-4 w-64" />
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------- the secrets helper

const HELPER_LINE: Record<string, string> = {
  off: "Password managers are off on this server.",
  starting: "Starting the secrets helper…",
  running: "The secrets helper is running.",
  unreachable: "The secrets helper is switched on but not answering.",
};
const HELPER_TONE: Record<string, string> = { off: "var(--ink-faint)", starting: "var(--warning)", running: "var(--good)", unreachable: "var(--critical)" };

/** The exact `.env` line and command, for a server with no updater service (or an admin who prefers it). */
function ManualSteps({ profile }: { profile: string }) {
  return (
    <div className="text-[12.5px] flex flex-col gap-1.5" style={{ color: "var(--ink-dim)" }} data-testid="helper-manual">
      <span>
        To switch them on by hand, add <code className="font-mono">{profile}</code> to <code className="font-mono">COMPOSE_PROFILES</code> in the server&apos;s <code className="font-mono">.env</code>, then start it:
      </span>
      <pre className="field px-3 py-2 font-mono text-[12px] whitespace-pre-wrap">{`COMPOSE_PROFILES=${profile}        # or https,${profile} when HTTPS is on\ndocker compose up -d`}</pre>
    </div>
  );
}

function Helper({ admin }: { admin: boolean }) {
  const status = useSecretsHelper();
  const request = useRequestSecretsHelper();
  const [error, setError] = useState<Failure | null>(null);
  const s: HelperStatus | undefined = status.data;
  if (!s) return status.isError ? <ErrorLine error={failure(status.error, "Could not load the secrets helper's status.")} /> : null;

  async function ask(enabled: boolean) {
    setError(null);
    try {
      await request.mutateAsync(enabled);
    } catch (e) {
      setError(failure(e, enabled ? "Could not ask for password managers to be switched on." : "Could not ask for password managers to be switched off."));
    }
  }

  const waiting = s.pending === "enable" ? "Waiting for the updater (up to 20 seconds)…" : s.pending === "disable" ? "Switching off: waiting for the updater (up to 20 seconds)…" : null;
  return (
    <div className="ledger px-4 py-3 flex flex-col gap-3" data-testid="secrets-helper">
      <div className="flex flex-wrap items-center gap-3">
        <span className="dot" style={{ background: HELPER_TONE[s.state] ?? "var(--ink-faint)" }} aria-hidden />
        <div className="min-w-0 flex-1 text-[13px]">
          <span role="status" data-state={s.state}>
            {HELPER_LINE[s.state] ?? s.state}
          </span>
          {s.state === "running" && s.version && <span style={{ color: "var(--ink-faint)" }}> Version {s.version}.</span>}
          <div className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {waiting ??
              (s.state === "off"
                ? "Until they are on, every secret reference fails at once and Test connection says the helper is off. Settings and references still work."
                : s.state === "unreachable"
                  ? "Secret references fail until it answers. Check the secrets-helper container and its logs."
                  : null)}
          </div>
        </div>
        {admin && s.can_request && !s.pending && (
          <div className="flex items-center gap-2">
            {(s.state === "off" || s.state === "unreachable") && !s.enabled && (
              <button type="button" className="btn btn-primary btn-sm" onClick={() => ask(true)} disabled={request.isPending} aria-busy={request.isPending}>
                <Power {...ICON} aria-hidden />
                Enable password managers
              </button>
            )}
            {s.enabled && (
              <ConfirmButton label="Disable password managers" confirmLabel="Disable" className="btn btn-sm" disabled={request.isPending} onConfirm={() => ask(false)}>
                Disable
              </ConfirmButton>
            )}
          </div>
        )}
      </div>
      {s.message && s.state !== "running" && <ErrorLine error={{ message: s.message }} />}
      {s.state === "off" && !admin && (
        <p className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
          An instance admin can switch them on here.
        </p>
      )}
      {s.state === "off" && admin && !s.can_request && <ManualSteps profile={s.profile} />}
      {admin && (s.state === "off" || s.state === "starting") && s.can_request && (
        <p className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
          Switching them on or off restarts the backend: the app is away for a few seconds.
        </p>
      )}
      {error && <ErrorLine error={error} />}
    </div>
  );
}

// ---------------------------------------------------------------- providers

function Providers({ manage }: { manage: boolean }) {
  const providers = useSecretProviders();
  const rows = providers.data ?? null;
  return (
    <>
      {providers.isError && <ErrorLine error={failure(providers.error, "Could not load the password managers.", " Reload the page to try again.")} />}
      <div className="ledger overflow-hidden">
        <div className="hairline-rows">
          {KINDS.map((k) => (
            <ProviderRow key={k.kind} spec={k} row={rows?.find((r) => r.kind === k.kind) ?? null} loading={rows === null && !providers.isError} manage={manage} />
          ))}
        </div>
      </div>
    </>
  );
}

function ProviderRow({ spec, row, loading, manage }: { spec: (typeof KINDS)[number]; row: ProviderOut | null; loading: boolean; manage: boolean }) {
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
  const kinds = useKindInfo();
  const info = kinds.info(spec.kind);
  const name = kinds.name(spec.kind);

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
          <div className="text-[13px] font-medium flex items-center gap-2">
            {name}
            <PreviewBadge info={info} />
          </div>
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
        {manage && !editing && (
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
                label={`Remove ${name}`}
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

      {info?.preview && info.note && (
        <p className="text-[12px] -mt-1" style={{ color: "var(--ink-dim)" }} data-testid="preview-note">
          {info.note}
        </p>
      )}
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

/** `vaults`: the ones the user may bind references to (all theirs for an org owner, the ones they own for a vault owner). */
function References({ vaults }: { vaults: { id: string; name: string }[] }) {
  const refs = useSecretRefs();
  const counts = useSecretCounts();
  const providers = useSecretProviders();
  const kinds = useKindInfo();
  const mine = useVaults().data ?? [];
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
    const personal = mine.find((v) => v.kind === "personal")?.id;
    setDraft({ ...emptyDraft, vault_id: vaults.find((v) => v.id === personal)?.id ?? vaults[0]?.id ?? "" });
  }

  function startEdit(r: RefOut) {
    setEditingId(r.id);
    setError(null);
    setDraft({ reference: r.reference, provider: r.provider, vault_id: r.vault_id, label: r.label === r.reference ? "" : r.label, tools: r.allowed_tools.join(", "), actors: r.allowed_actors.join(", ") });
  }

  async function save() {
    if (!draft) return;
    setError(null);
    // eu:// handles are secrets Eunomia holds (no password manager, no reference to register)
    if (draft.reference.trim().toLowerCase().startsWith("eu://")) {
      setError({ message: "An eu:// handle is a secret Eunomia holds, not a password manager reference. It needs no reference: find it under Held secrets." });
      return;
    }
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
                options={KINDS.map((k) => ({
                  value: k.kind,
                  label: kinds.name(k.kind),
                  hint: [configured.includes(k.kind) ? "configured" : "", kinds.info(k.kind)?.preview ? "preview" : ""].filter(Boolean).join(", ") || undefined,
                }))}
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
                      {kinds.name(r.provider)}
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
  const log = useSecretActivity();
  const entries = log.data?.entries ?? null;
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-[13px] font-medium mt-2">Resolution log</h3>
      <p className="text-[12.5px] -mt-1" style={{ color: "var(--ink-dim)" }}>
        Every time a tool asked for a secret, and every secret request, fill, transform and outbound call: who, which reference or handle, which tool, and the outcome. Never the value. A vault owner sees the vaults they own. Tool calls themselves are under Settings, Usage.
      </p>
      {log.isError && <ErrorLine error={failure(log.error, "Could not load the resolution log.", " Reload the page to try again.")} />}
      <div className="ledger overflow-hidden">
        <table className="w-full text-[13px]">
          <thead>
            <tr className="label border-b text-left h-8">
              <th className="px-4 font-medium">When</th>
              <th className="px-3 font-medium">What</th>
              <th className="px-3 font-medium">Reference</th>
              <th className="px-3 font-medium hidden md:table-cell">Tool</th>
              <th className="px-3 font-medium hidden lg:table-cell">Who</th>
              <th className="px-3 font-medium">Outcome</th>
            </tr>
          </thead>
          <tbody className="hairline-rows">
            {entries === null && !log.isError && (
              <tr aria-hidden>
                <td colSpan={6} className="px-4 h-10">
                  <span className="skeleton h-3 w-40" />
                </td>
              </tr>
            )}
            {entries?.length === 0 && (
              <tr>
                <td colSpan={6} className="px-4 h-14" style={{ color: "var(--ink-dim)" }}>
                  No secret has been resolved yet.
                </td>
              </tr>
            )}
            {entries?.map((e) => (
              <tr key={e.id} className="h-10" data-testid="secret-log-row">
                <td className="px-4 whitespace-nowrap" style={{ color: "var(--ink-dim)" }}>
                  {relativeTime(e.at)}
                </td>
                <td className="px-3 whitespace-nowrap">{ACTION_LABEL[e.action] ?? e.action}</td>
                <td className="px-3 font-mono text-[12.5px] truncate max-w-[280px]" title={e.host ? `to ${e.host}` : undefined}>
                  {e.reference || e.lineage.join(", ")}
                </td>
                <td className="px-3 hidden md:table-cell font-mono text-[12px]">{e.tool}</td>
                <td className="px-3 hidden lg:table-cell font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {e.actor_id} · {e.channel}
                </td>
                <td className="px-3">
                  <span className="inline-flex items-center gap-1.5" title={e.reason || undefined}>
                    <span className="dot" style={{ background: OUTCOME_COLOUR[e.outcome] ?? "var(--ink-faint)" }} aria-hidden />
                    {CODE_LABEL[e.code] ?? OUTCOME_LABEL[e.outcome] ?? e.outcome}
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
