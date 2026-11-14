"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useRef, useState } from "react";
import { KeyIcon, PencilSimpleIcon, XIcon } from "@phosphor-icons/react/ssr";
import { useCreateToken, useRenameToken, useRevokeToken, useTokens } from "@/lib/queries/auth";
import { useVaults } from "@/lib/queries/vaults";
import type { ApiToken, Scope, Vault } from "@/lib/types";
import Tooltip from "@/components/bits/Tooltip";
import { ICON, CopyField, relativeTime, expiryLabel, PanelHead, RevokeButton, TableSkeleton } from "./shared";

const SCOPES: { id: Scope; label: string; hint: string }[] = [
  { id: "memory:read", label: "Read memories", hint: "Look up and read memories, people & topics and memory spaces" },
  { id: "memory:write", label: "Save memories", hint: "Add, change and delete memories and people & topics" },
  { id: "vaults:admin", label: "Manage memory spaces", hint: "Create, rename, share and delete memory spaces; settings, tokens and chat" },
  { id: "connectors", label: "Connected sources", hint: "View and sync connected sources" },
];

const DEFAULT_SCOPES: Scope[] = ["memory:read", "memory:write"];

// Same limit as the API (and as a vault name).
const NAME_MAX = 80;

/* Below 640px a row is a wrapping flex box and its cells lose the table's height, padding and rules. */
const PHONE_ROW = "max-sm:flex max-sm:flex-wrap max-sm:items-center max-sm:gap-x-2 max-sm:gap-y-1 max-sm:px-3 max-sm:py-3 max-sm:border-b max-sm:border-[var(--border)] max-sm:last:border-b-0";
const PHONE_CELL = "max-sm:block max-sm:h-auto! max-sm:p-0! max-sm:border-0!";

const EXPIRY_OPTIONS = [
  { value: "", label: "Never expires" },
  { value: "7", label: "7 days" },
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "365", label: "1 year" },
];

export function TokensSection() {
  const tokensQuery = useTokens();
  const tokens = tokensQuery.data ?? (tokensQuery.isError ? [] : null);
  const createToken = useCreateToken();
  const revokeToken = useRevokeToken();
  const renameToken = useRenameToken();
  const vaultsQuery = useVaults();
  const vaultList: Vault[] = vaultsQuery.data ?? [];
  const [name, setName] = useState("");
  // Least privilege: a new token reads and saves memories only. Managing spaces and sources is opt-in.
  const [scopes, setScopes] = useState<Scope[]>(DEFAULT_SCOPES);
  // Untouched (null) means the safer default: your personal space, not every space. "All my memory spaces" stays one click away.
  const [vaultChoice, setVaultChoice] = useState<string | null>(null);
  const personal = vaultList.find((v) => v.is_default && v.id);
  const vaultId = vaultChoice ?? personal?.id;
  const destinationReady = vaultsQuery.isSuccess && vaultId !== undefined
    && (vaultChoice === "" || vaultList.some((v) => v.id === vaultId));
  const setVaultId = setVaultChoice;
  // New tokens expire after 90 days unless you pick otherwise.
  const [expiryDays, setExpiryDays] = useState("90");
  const [minted, setMinted] = useState<{ name: string; token: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  // Two-step verification: a new token needs a recent second factor; the server asks (auth.step_up_required) and we show a code field.
  const [stepUp, setStepUp] = useState(false);
  const [mfaCode, setMfaCode] = useState("");
  // Inline rename: the token being renamed and its draft. The ref is the source of truth for
  // "still editing", so Enter followed by the blur it causes (or Esc) saves at most once.
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const renamingRef = useRef<string | null>(null);

  const vaultName = (id: string) => vaultList.find((v) => v.id === id)?.name ?? id;

  async function create() {
    if (!name.trim() || !destinationReady || vaultId === undefined) return;
    setBusy(true);
    setError(null);
    try {
      const res = await createToken.mutateAsync({
        name: name.trim(),
        scopes,
        vault_id: vaultId === "" ? null : vaultId,
        expires_at: expiryDays ? new Date(Date.now() + Number(expiryDays) * 86_400_000).toISOString() : null,
        ...(stepUp && mfaCode.trim() ? { mfa_code: mfaCode.trim() } : {}),
      });
      setMinted({ name: res.name, token: res.token });
      setName("");
      setStepUp(false);
      setMfaCode("");
    } catch (e) {
      const f = failure(e, "Could not create a token. Check that you're still signed in, then try again.");
      if (f.code === "auth.step_up_required") {
        // not an error the person made: ask for the code in place
        setStepUp(true);
      } else {
        setError(f);
      }
      setMfaCode("");
    } finally {
      setBusy(false);
    }
  }

  function startRename(t: ApiToken) {
    renamingRef.current = t.id;
    setRenaming(t.id);
    setDraft(t.name);
    setError(null);
  }

  function stopRename() {
    renamingRef.current = null;
    setRenaming(null);
  }

  async function saveRename(t: ApiToken) {
    if (renamingRef.current !== t.id) return;
    const next = draft.trim();
    if (!next || next === t.name) return stopRename();
    renamingRef.current = null;
    setError(null);
    try {
      await renameToken.mutateAsync({ token_id: t.id, name: next });
      setRenaming(null);
    } catch (e) {
      renamingRef.current = t.id;
      setError(failure(e, "Could not rename that token. Reload and try again."));
    }
  }

  async function revoke(id: string) {
    setError(null);
    try {
      await revokeToken.mutateAsync(id);
    } catch (e) {
      setError(failure(e, "Could not revoke that token. Reload and try again."));
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="API tokens">
        A token lets an AI app or script use Eunomia as you. It is shown once, when you create it.
      </PanelHead>

      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          create();
        }}
      >
        <div className="flex flex-col gap-1.5">
          <label htmlFor="token-name" className="label">
            Token name
          </label>
          <input
            id="token-name"
            className="field h-10 px-3 text-[14px] w-full max-w-[360px]"
            placeholder="For example: work laptop"
            maxLength={NAME_MAX}
            value={name}
            required
            aria-describedby={name.trim() ? undefined : "token-name-hint"}
            onChange={(e) => setName(e.target.value)}
          />
          {!name.trim() && (
            <p id="token-name-hint" className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
              Name it after where you&apos;ll use it, so you can tell your tokens apart later.
            </p>
          )}
        </div>

        <div className="flex flex-col gap-1.5">
          <span id="token-scopes" className="label">
            What it can do
          </span>
          <div className="flex flex-col gap-2" role="group" aria-labelledby="token-scopes">
            {SCOPES.map((s) => {
              const on = scopes.includes(s.id);
              return (
                <label key={s.id} className="flex items-start gap-2.5 cursor-pointer text-[14px]">
                  <input
                    type="checkbox"
                    className="mt-[3px] h-4 w-4 shrink-0 cursor-pointer"
                    style={{ accentColor: "var(--accent)" }}
                    checked={on}
                    onChange={() => setScopes((cur) => (on ? cur.filter((x) => x !== s.id) : [...cur, s.id]))}
                  />
                  <span className="flex flex-col">
                    <span style={{ color: "var(--ink)" }}>{s.label}</span>
                    <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                      {s.hint}
                    </span>
                  </span>
                </label>
              );
            })}
          </div>
          {scopes.length === 0 && (
            <span className="text-[12px]" style={{ color: "var(--critical)" }}>
              Choose at least one scope.
            </span>
          )}
        </div>

        <div className="flex flex-wrap items-end gap-3">
          <div className="flex flex-col gap-1.5 w-full sm:w-[220px]">
            <span className="label">
              Memory space
            </span>
            <Select
              aria-label="Restrict to a memory space"
              value={vaultId ?? "unresolved"}
              disabled={!vaultsQuery.isSuccess}
              placeholder="Loading memory spaces…"
              onChange={setVaultId}
              options={[
                { value: "", label: "All my memory spaces" },
                ...vaultList.map((v) => ({ value: v.id, label: v.name })),
              ]}
              className="h-10 text-[14px] w-full"
            />
          </div>
          <div className="flex flex-col gap-1.5 w-full sm:w-[160px]">
            <span className="label">
              Expiry
            </span>
            <Select
              aria-label="Expiry"
              value={expiryDays}
              onChange={setExpiryDays}
              options={EXPIRY_OPTIONS}
              className="h-10 text-[14px] w-full"
            />
          </div>
          {stepUp && (
            <div className="flex flex-col gap-1.5 w-full sm:w-[200px]">
              <label htmlFor="token-mfa-code" className="label">
                Authenticator code
              </label>
              <input
                id="token-mfa-code"
                className="field h-10 px-3 text-[14px] w-full font-mono"
                inputMode="numeric"
                autoComplete="one-time-code"
                autoFocus
                placeholder="123456"
                value={mfaCode}
                onChange={(e) => setMfaCode(e.target.value)}
              />
            </div>
          )}
          <button type="submit" disabled={busy || !destinationReady || scopes.length === 0 || !name.trim() || (stepUp && !mfaCode.trim())} className="btn btn-primary">
            <KeyIcon {...ICON} />
            {busy ? "Creating…" : "Create token"}
          </button>
        </div>
        {!destinationReady && (
          <p role="status" className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {vaultsQuery.isError ? "Could not load your memory spaces. Reload to try again."
              : vaultsQuery.isPending ? "Loading your Personal memory space…"
              : "Your Personal memory space is unavailable. Choose a destination before creating a token."}
          </p>
        )}
        {stepUp && (
          <p role="status" className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            Confirm it&apos;s you: enter the code from your authenticator app (or a recovery code) to create this token.
          </p>
        )}
        {vaultId && (
          <p className="text-[12px] -mt-1" style={{ color: "var(--ink-faint)" }}>
            A token limited to one memory space works only there. It can&apos;t create memory spaces or change account settings.
          </p>
        )}
      </form>

      {minted && (
        <div role="status" className="task-panel pop-in p-5 flex flex-col gap-3" style={{ transformOrigin: "top center" }}>
          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="text-[16px] font-semibold">Token “{minted.name}” created</div>
              <p className="text-[14px] mt-0.5" style={{ color: "var(--ink)" }}>
                Copy it now and keep it somewhere safe, like a password manager. It won&apos;t be shown again.
              </p>
            </div>
            <button onClick={() => setMinted(null)} aria-label="Dismiss" className="btn btn-ghost btn-sm btn-icon">
              <XIcon {...ICON} />
            </button>
          </div>
          <CopyField value={minted.token} reveal />
        </div>
      )}

      {tokensQuery.isError && <ErrorLine error={failure(tokensQuery.error, "Could not load your tokens.", " Reload the page to try again.")} />}
      {error && <ErrorLine error={error} />}

      {/* Phones: each token is a stacked entry (name and Revoke, then what it can do, then dates), never a sideways scroll. */}
      <div className="ledger sm:overflow-x-auto">
        <table className="data-table max-sm:block">
          <thead className="max-sm:hidden">
            <tr>
              <th>Name</th>
              <th>Can</th>
              <th className="w-[100px]">Last used</th>
              <th className="w-[90px]">Expires</th>
              <th className="w-[110px]">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody className="max-sm:block">
            {tokens === null && <TableSkeleton cols={5} />}
            {tokens?.length === 0 && (
              <tr className="max-sm:block">
                <td colSpan={5} className={`${PHONE_CELL} max-sm:px-3! max-sm:py-4!`} style={{ color: "var(--ink-dim)", height: 56 }}>
                  No tokens yet. Create one above to connect an AI app with a token.
                </td>
              </tr>
            )}
            {tokens?.map((t) => {
              const exp = expiryLabel(t.expires_at);
              const full = SCOPES.every((s) => t.scopes.includes(s.id));
              return (
                <tr key={t.id} className={PHONE_ROW}>
                  <td className={`font-medium ${PHONE_CELL} max-sm:min-w-0 max-sm:flex-1`}>
                    {renaming === t.id ? (
                      <input
                        aria-label="New token name"
                        autoFocus
                        maxLength={NAME_MAX}
                        className="field h-9 px-2 text-[14px] w-full min-w-[140px]"
                        value={draft}
                        disabled={renameToken.isPending}
                        onChange={(e) => setDraft(e.target.value)}
                        onBlur={() => saveRename(t)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            saveRename(t);
                          } else if (e.key === "Escape") {
                            stopRename();
                          }
                        }}
                      />
                    ) : (
                      <span className="flex items-center gap-1 min-w-0">
                        <button type="button" onClick={() => startRename(t)} className="truncate text-left cursor-text">
                          {t.name}
                        </button>
                        <Tooltip label="Rename token">
                          <button
                            type="button"
                            onClick={() => startRename(t)}
                            aria-label={`Rename ${t.name}`}
                            className="btn btn-ghost btn-sm btn-icon shrink-0"
                          >
                            <PencilSimpleIcon {...ICON} />
                          </button>
                        </Tooltip>
                      </span>
                    )}
                  </td>
                  <td className={`${PHONE_CELL} max-sm:order-last max-sm:basis-full`}>
                    <span className="flex flex-col gap-0.5 py-1.5 max-sm:py-0 min-w-0">
                      {full ? (
                        <span>Full access</span>
                      ) : (
                        <span>{t.scopes.map((sc) => SCOPES.find((x) => x.id === sc)?.label ?? sc).join(", ")}</span>
                      )}
                      <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                        {t.vault_id ? `Memory space: ${vaultName(t.vault_id)}` : "All memory spaces"}
                      </span>
                    </span>
                  </td>
                  <td className={`tabular-nums text-[13px] ${PHONE_CELL} max-sm:order-last max-sm:text-[12px]`} style={{ color: "var(--ink-dim)" }}>
                    <span className="sm:hidden">Last used </span>
                    {relativeTime(t.last_used_at)}
                  </td>
                  <td
                    className={`tabular-nums text-[13px] whitespace-nowrap ${PHONE_CELL} max-sm:order-last max-sm:text-[12px]`}
                    style={{ color: exp.expired ? "var(--critical)" : "var(--ink-dim)" }}
                    title={t.expires_at ? new Date(t.expires_at).toLocaleString() : undefined}
                  >
                    {/* the column head says "Expires": the cell is just "in 90d", which never wraps */}
                    <span className="sm:hidden">· Expires </span>
                    {exp.text}
                  </td>
                  <td className={`text-right ${PHONE_CELL}`}>
                    <RevokeButton label={t.name} onRevoke={() => revoke(t.id)} />
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
