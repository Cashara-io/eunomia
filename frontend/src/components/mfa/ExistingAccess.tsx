"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useRevokeToken, useTokens } from "@/lib/queries/auth";
import { useOauthGrants, useRevokeOauthGrant } from "@/lib/queries/oauth";

const day = (iso: string | null | undefined) => (iso ? new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : "unknown date");

/** After enrolment: the API tokens and connected apps that already exist keep working without a code. List
 * them so the person can revoke any they don't recognise. Nothing is revoked for them: that would cut off
 * their AI apps. */
export function ExistingAccess() {
  const tokens = useTokens().data ?? [];
  const apps = useOauthGrants().data ?? [];
  const revokeToken = useRevokeToken();
  const revokeApp = useRevokeOauthGrant();
  const [error, setError] = useState<Failure | null>(null);
  if (tokens.length === 0 && apps.length === 0) return null;

  async function revoke(fn: () => Promise<unknown>) {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(failure(e, "Could not revoke it. Try again from Settings."));
    }
  }

  const rows = [
    ...tokens.map((t) => ({ key: `t-${t.id}`, kind: "API token", name: t.name, created: t.created_at, revoke: () => revokeToken.mutateAsync(t.id) })),
    ...apps.map((a) => ({ key: `a-${a.id}`, kind: "Connected app", name: a.client_name, created: a.created_at, revoke: () => revokeApp.mutateAsync(a.id) })),
  ];
  return (
    <div className="flex flex-col gap-2" data-testid="mfa-existing-access">
      <p className="text-reading" style={{ color: "var(--ink)" }}>
        These already have access and keep working without a code. Revoke any you don&apos;t recognise.
      </p>
      <ul className="ledger hairline-rows max-w-[560px]">
        {rows.map((r) => (
          <li key={r.key} className="px-4 py-2.5 flex items-center justify-between gap-3">
            <span className="flex flex-col min-w-0">
              <span className="text-body font-semibold truncate">{r.name}</span>
              <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                {r.kind} · created {day(r.created)}
              </span>
            </span>
            <button type="button" className="btn btn-ghost btn-sm" onClick={() => revoke(r.revoke)} aria-label={`Revoke ${r.kind.toLowerCase()} ${r.name}`}>
              Revoke
            </button>
          </li>
        ))}
      </ul>
      {error && <ErrorLine error={error} />}
    </div>
  );
}
