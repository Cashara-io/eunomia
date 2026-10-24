"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import { KeyRound, Monitor } from "lucide-react";
import { useMe, useRevokeSession, useSessions } from "@/lib/queries/auth";
import { ICON, relativeTime, PanelHead, RevokeButton, TableSkeleton } from "./shared";

export function SessionsSection() {
  const sessionsQuery = useSessions();
  const sessions = sessionsQuery.data ?? (sessionsQuery.isError ? [] : null);
  const revokeSession = useRevokeSession();
  const me = useMe().data;
  const [error, setError] = useState<Failure | null>(null);

  async function revoke(id: string) {
    setError(null);
    try {
      await revokeSession.mutateAsync(id);
    } catch (e) {
      setError(failure(e, "Could not revoke that session. Reload and try again."));
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="Active sessions">Browser sign-ins. Revoking one signs that browser out immediately.</PanelHead>
      {me?.oidc_provider && (
        <div className="flex items-center gap-2 text-[13px]" style={{ color: "var(--ink-dim)" }}>
          <KeyRound {...ICON} className="shrink-0" aria-hidden />
          <span>
            Signed in with <strong style={{ color: "var(--ink)" }}>{me.oidc_provider}</strong> as {me.email}.
            {!me.has_password && " This account has no password: sign-in always goes through the provider."}
          </span>
        </div>
      )}
      {sessionsQuery.isError && <ErrorLine error={failure(sessionsQuery.error, "Could not load your sessions.", " Reload the page to try again.")} />}
      {error && <ErrorLine error={error} />}
      <div className="ledger overflow-x-auto">
        <table className="data-table">
          <thead>
            <tr>
              <th>Device</th>
              <th className="w-[120px]">Signed in</th>
              <th className="w-[120px]">Last seen</th>
              <th className="w-[220px]">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {sessions === null && <TableSkeleton cols={4} />}
            {sessions?.length === 0 && (
              <tr>
                <td colSpan={4} style={{ color: "var(--ink-dim)", height: 56 }}>
                  No active sessions.
                </td>
              </tr>
            )}
            {sessions?.map((s) => (
              <tr key={s.id}>
                <td>
                  <span className="flex items-center gap-2 min-w-0">
                    <Monitor {...ICON} className="shrink-0" style={{ color: "var(--ink-faint)" }} aria-hidden />
                    <span className="truncate max-w-[360px]" title={s.user_agent}>
                      {s.user_agent || "Unknown device"}
                    </span>
                    {s.current && <span className="pill shrink-0">This device</span>}
                  </span>
                </td>
                <td className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {relativeTime(s.created_at)}
                </td>
                <td className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {relativeTime(s.last_seen_at)}
                </td>
                <td className="text-right">
                  <RevokeButton label="this session" onRevoke={() => revoke(s.id)} warn={s.current ? "Signs you out here." : undefined} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
