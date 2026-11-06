"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import Link from "next/link";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import { useOauthGrants, useRevokeOauthGrant } from "@/lib/queries/oauth";
import EmptyState from "@/components/ui/EmptyState";
import { relativeTime, PanelHead, QUIET_DANGER, TableSkeleton } from "./shared";

const SCOPE_WORDS: Record<string, string> = {
  "memory:read": "read memories",
  "memory:write": "save memories",
  "vaults:admin": "manage memory spaces",
  connectors: "manage connected sources",
};

const sentence = (w: string) => w.charAt(0).toUpperCase() + w.slice(1);

export function ConnectedAppsSection() {
  const grantsQuery = useOauthGrants();
  const grants = grantsQuery.data ?? (grantsQuery.isError ? [] : null);
  const revokeGrant = useRevokeOauthGrant();
  const [error, setError] = useState<Failure | null>(null);
  // The app waiting for the person to confirm Disconnect.
  const [pending, setPending] = useState<{ id: string; name: string } | null>(null);

  async function revoke(id: string) {
    setError(null);
    try {
      await revokeGrant.mutateAsync(id);
      setPending(null);
    } catch (e) {
      setPending(null);
      setError(failure(e, "Could not disconnect that app. Reload and try again."));
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="Connected apps">
        AI apps you allowed to use Eunomia. Disconnecting one stops it at once; your memories stay.
      </PanelHead>
      {grantsQuery.isError && <ErrorLine error={failure(grantsQuery.error, "Could not load your connected apps.", " Reload the page to try again.")} />}
      {error && <ErrorLine error={error} />}
      {grants?.length === 0 ? (
        <EmptyState
          title="No AI apps are connected yet."
          action={
            <Link href="/" className="btn">
              Connect an AI app
            </Link>
          }
        >
          Choose your AI app on Home and follow its steps.
        </EmptyState>
      ) : (
      <div className="ledger overflow-x-auto">
        <table className="data-table">
          <thead>
            <tr>
              <th>App</th>
              <th>Access</th>
              <th className="w-[120px]">Connected</th>
              <th className="w-[120px]">Last used</th>
              <th className="w-[130px]">
                Action
              </th>
            </tr>
          </thead>
          <tbody>
            {grants === null && <TableSkeleton cols={5} />}
            {grants?.map((g) => (
              <tr key={g.id}>
                <td className="font-medium">
                  <span className="truncate max-w-[220px] block" title={g.client_id}>
                    {g.client_name}
                  </span>
                </td>
                <td>{sentence(g.scope.map((s) => SCOPE_WORDS[s] ?? s).join(", "))}</td>
                <td className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {relativeTime(g.created_at)}
                </td>
                <td className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {relativeTime(g.last_used_at)}
                </td>
                <td className="text-right">
                  <button type="button" className={QUIET_DANGER} onClick={() => setPending({ id: g.id, name: g.client_name })} aria-label={`Disconnect ${g.client_name}`}>
                    Disconnect…
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      )}
      {grants && grants.length > 0 && (
        <Link href="/" className="btn self-start">
          Connect another AI app
        </Link>
      )}
      <ConfirmDialog
        open={pending !== null}
        title={`Disconnect ${pending?.name ?? "this app"}?`}
        confirmLabel={`Disconnect ${pending?.name ?? "app"}`}
        danger
        busy={revokeGrant.isPending}
        onCancel={() => setPending(null)}
        onConfirm={() => pending && revoke(pending.id)}
      >
        <p>
          {pending?.name ?? "This app"} will stop being able to read or save your memories right away. Your saved memories
          stay in Eunomia. To use it again, you will need to connect it and approve access again.
        </p>
      </ConfirmDialog>
    </div>
  );
}
