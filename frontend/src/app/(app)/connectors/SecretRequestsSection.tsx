"use client";

import { useState } from "react";
import Link from "next/link";
import { ArrowsClockwiseIcon, KeyIcon, TrashIcon } from "@phosphor-icons/react/ssr";
import type { HeldOut, RequestOut } from "@/lib/gen";
import { useDeclineSecretRequest, useDeleteHeldSecret, useHeldSecrets, useReplaceHeldSecret, useSecretRequests } from "@/lib/queries/secretRequests";
import { useVaults } from "@/lib/queries/vaults";
import { relativeTime } from "@/lib/connectorMeta";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import ConfirmButton from "@/components/ConfirmButton";
import { askedBy } from "@/components/SecretRequestForm";
import { ICON } from "../settings/shared";

const STATUS_COLOUR: Record<string, string> = { pending: "var(--warning)", filled: "var(--good)", declined: "var(--ink-faint)", expired: "var(--ink-faint)" };

/**
 * Settings, Connectors: secrets agents asked for (docs/secret-requests.md). Requests waiting or answered,
 * and the secrets Eunomia holds for agents, by handle: replace or delete them, never read them.
 */
export default function SecretRequestsSection() {
  return (
    <section className="flex flex-col gap-3" aria-labelledby="secret-requests-heading">
      <div className="min-w-0">
        <h2 id="secret-requests-heading" className="section-title">
          Secrets agents asked for
        </h2>
        <p className="text-[12.5px] mt-1 max-w-[66ch]" style={{ color: "var(--ink-dim)" }}>
          When an agent needs a key it asks here instead of in the chat. You paste it in a pop-up; agents get a handle such as <code className="font-mono">eu://…/stripe_api_key</code> and can use it only with the hosts you
          approved. No one can read a value back: replace or delete it.
        </p>
      </div>
      <Requests />
      <Held />
    </section>
  );
}

function Requests() {
  const requests = useSecretRequests();
  const decline = useDeclineSecretRequest();
  const [error, setError] = useState<Failure | null>(null);
  const rows = (requests.data ?? []).slice(0, 20);
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-[13px] font-medium">Requests</h3>
      {requests.isError && <ErrorLine error={failure(requests.error, "Could not load the requests.", " Reload the page to try again.")} />}
      {error && <ErrorLine error={error} />}
      <div className="ledger overflow-hidden">
        <div className="hairline-rows">
          {requests.isLoading && (
            <div className="px-4 h-10 flex items-center" aria-hidden>
              <span className="skeleton h-3 w-48" />
            </div>
          )}
          {requests.data && rows.length === 0 && (
            <p className="px-4 py-4 text-[13px]" style={{ color: "var(--ink-dim)" }}>
              No requests yet. An agent asks with the <code className="font-mono">secret_request</code> tool.
            </p>
          )}
          {rows.map((r: RequestOut) => (
            <div key={r.id} className="px-4 py-2.5 flex flex-wrap items-center gap-3 text-[13px]" data-testid="secret-request-row">
              <span className="dot" style={{ background: STATUS_COLOUR[r.status] ?? "var(--ink-faint)" }} aria-hidden />
              <div className="min-w-0 flex-1">
                <div className="font-medium truncate">{r.label || r.name}</div>
                <div className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
                  {r.status} · {askedBy(r)} · {relativeTime(r.created_at)}
                </div>
              </div>
              {r.status === "pending" && r.can_fill && (
                <Link className="btn btn-sm" href={`/secrets/requests/${r.id.replace(/^secret_request:/, "")}`}>
                  Open
                </Link>
              )}
              {r.status === "pending" && r.can_decline && (
                <button type="button" className="btn btn-ghost btn-sm" disabled={decline.isPending} onClick={() => decline.mutateAsync(r.id).catch((e) => setError(failure(e, "Could not decline it. Try again.")))}>
                  Decline
                </button>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function Held() {
  const held = useHeldSecrets();
  const vaults = useVaults().data ?? [];
  const remove = useDeleteHeldSecret();
  const [error, setError] = useState<Failure | null>(null);
  const vaultName = (id: string) => vaults.find((v) => v.id === id)?.name ?? id;
  const rows = held.data ?? null;
  const names = new Map((rows ?? []).map((s) => [s.id, s.name]));
  return (
    <div className="flex flex-col gap-2">
      <h3 className="text-[13px] font-medium mt-2">Held secrets</h3>
      {held.isError && <ErrorLine error={failure(held.error, "Could not load the held secrets.", " Reload the page to try again.")} />}
      {error && <ErrorLine error={error} />}
      <div className="ledger overflow-hidden">
        <table className="w-full text-[13px]">
          <thead>
            <tr className="label border-b text-left h-8">
              <th className="px-4 font-medium">Handle</th>
              <th className="px-3 font-medium hidden md:table-cell">Vault</th>
              <th className="px-3 font-medium hidden md:table-cell">May be sent to</th>
              <th className="px-3">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody className="hairline-rows">
            {rows?.length === 0 && (
              <tr>
                <td colSpan={4} className="px-4 h-14" style={{ color: "var(--ink-dim)" }}>
                  None yet.
                </td>
              </tr>
            )}
            {rows?.map((s: HeldOut) => (
              <HeldRow key={s.id} s={s} vault={vaultName(s.vault_id)} parents={s.derived_from.map((p) => names.get(p) ?? p)} onError={setError} removing={remove.isPending} onRemove={() => remove.mutateAsync(s.id)} />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function HeldRow({ s, vault, parents, onError, removing, onRemove }: { s: HeldOut; vault: string; parents: string[]; onError: (f: Failure) => void; removing: boolean; onRemove: () => Promise<unknown> }) {
  const replace = useReplaceHeldSecret();
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState("");
  async function save() {
    try {
      await replace.mutateAsync({ id: s.id, value });
      setValue("");
      setEditing(false);
    } catch (e) {
      onError(failure(e, "Could not replace it. Check the value and try again."));
    }
  }
  return (
    <tr className="align-top" data-testid="held-secret-row">
      <td className="px-4 py-2.5 min-w-0">
        <div className="flex items-center gap-1.5 font-mono text-[12.5px] break-all">
          <KeyIcon {...ICON} aria-hidden style={{ color: "var(--ink-faint)", flexShrink: 0 }} />
          {s.handle}
        </div>
        <div className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
          {s.op ? `${s.op} of ${parents.join(", ")}` : "pasted"} · {relativeTime(s.updated_at)}
          {s.expires_at ? ` · expires ${new Date(s.expires_at).toLocaleDateString()}` : ""}
        </div>
        {editing && (
          <form
            className="flex items-center gap-2 mt-2"
            onSubmit={(e) => {
              e.preventDefault();
              save();
            }}
          >
            <input type="password" autoComplete="off" spellCheck={false} aria-label={`New value for ${s.name}`} className="field h-8 px-3 text-[13px] font-mono flex-1" value={value} onChange={(e) => setValue(e.target.value)} />
            <button type="submit" className="btn btn-primary btn-sm" disabled={!value.trim() || replace.isPending}>
              Save
            </button>
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </form>
        )}
      </td>
      <td className="px-3 py-2.5 hidden md:table-cell">{vault}</td>
      <td className="px-3 py-2.5 hidden md:table-cell font-mono text-[12px]">{s.allowed_hosts.length ? s.allowed_hosts.join(", ") : <span style={{ color: "var(--ink-faint)" }}>nowhere</span>}</td>
      <td className="px-3 py-2.5">
        {s.can_manage && (
          <div className="flex items-center justify-end gap-1">
            {!s.op && (
              <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label={`Replace ${s.name}`} onClick={() => setEditing(true)}>
                <ArrowsClockwiseIcon {...ICON} />
              </button>
            )}
            <ConfirmButton label={`Delete ${s.name}`} confirmLabel="Delete" className="btn btn-ghost btn-sm btn-icon" disabled={removing} onConfirm={() => onRemove().catch((e) => onError(failure(e, "Could not delete it. Try again.")))}>
              <TrashIcon {...ICON} />
            </ConfirmButton>
          </div>
        )}
      </td>
    </tr>
  );
}
