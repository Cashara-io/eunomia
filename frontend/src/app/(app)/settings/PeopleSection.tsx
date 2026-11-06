"use client";

import { useState } from "react";
import { X } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Tooltip from "@/components/bits/Tooltip";
import { usePeople, useSetAdmin, useSetSuspended } from "@/lib/queries/admin";
import type { Person } from "@/lib/gen";
import { ICON, PanelHead, TableSkeleton, relativeTime } from "./shared";

/* Why an admin can't be changed here, for the tooltip on the disabled button. */
const LOCKED: Record<string, string> = {
  creator: "This account set up the workspace, so it is always an admin.",
  operator: "The server's settings list this account as an admin. Change it there.",
};

const date = (iso?: string | null) => (iso ? new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "—");

/* A disabled button with a reason: the tooltip sits on a wrapper because a disabled button gets no hover. */
function Locked({ label, reason }: { label: string; reason: string }) {
  return (
    <Tooltip label={reason}>
      <span tabIndex={0} aria-label={`${label}: ${reason}`} className="inline-flex">
        <button type="button" className="btn btn-ghost btn-sm" disabled aria-hidden tabIndex={-1}>
          {label}
        </button>
      </span>
    </Tooltip>
  );
}

function RowActions({ p, onError }: { p: Person; onError: (f: Failure | null) => void }) {
  const setAdmin = useSetAdmin();
  const setSuspended = useSetSuspended();
  const [confirming, setConfirming] = useState(false);
  const busy = setAdmin.isPending || setSuspended.isPending;
  const suspended = !!p.suspended_at;
  const locked = LOCKED[p.admin];

  async function run(action: () => Promise<unknown>, fallback: string) {
    onError(null);
    try {
      await action();
    } catch (e) {
      onError(failure(e, fallback));
    } finally {
      setConfirming(false);
    }
  }

  const toggleAdmin = () =>
    run(
      () => setAdmin.mutateAsync({ user_id: p.id, admin: p.role !== "admin" }),
      p.role === "admin" ? `Could not remove admin rights from ${p.email}.` : `Could not make ${p.email} an admin.`
    );
  const toggleSuspended = () =>
    run(
      () => setSuspended.mutateAsync({ user_id: p.id, suspended: !suspended }),
      suspended ? `Could not restore ${p.email}.` : `Could not suspend ${p.email}.`
    );

  if (confirming)
    return (
      <span className="inline-flex flex-wrap items-center justify-end gap-1">
        <span className="text-[13px] mr-1" style={{ color: "var(--ink-dim)" }}>
          Signs them out everywhere
        </span>
        <button type="button" className="btn btn-danger btn-sm" disabled={busy} onClick={toggleSuspended}>
          {busy ? "Suspending…" : "Suspend"}
        </button>
        <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label="Keep access" onClick={() => setConfirming(false)}>
          <X {...ICON} />
        </button>
      </span>
    );

  return (
    <span className="inline-flex flex-wrap items-center justify-end gap-1">
      {locked ? (
        <Locked label="Remove admin" reason={locked} />
      ) : p.is_you && p.role === "admin" ? (
        <Locked label="Remove admin" reason="You can't remove your own admin rights. Ask another admin." />
      ) : (
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={toggleAdmin}>
          {p.role === "admin" ? "Remove admin" : "Make admin"}
        </button>
      )}
      {p.is_you ? (
        <Locked label="Suspend" reason="You can't suspend your own account." />
      ) : p.admin === "creator" ? (
        <Locked label="Suspend" reason="This account set up the workspace, so it can't be suspended." />
      ) : suspended ? (
        <button type="button" className="btn btn-sm" disabled={busy} onClick={toggleSuspended}>
          Restore
        </button>
      ) : (
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setConfirming(true)}>
          Suspend
        </button>
      )}
    </span>
  );
}

export function PeopleSection() {
  const query = usePeople();
  const data = query.data;
  const people = data?.people ?? null;
  const signup = data?.signup;
  const [actionError, setActionError] = useState<Failure | null>(null);

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="People">Everyone with an account in this workspace.</PanelHead>

      {query.isError && <ErrorLine error={failure(query.error, "Could not load the people in this workspace. Reload to try again.")} />}

      {signup && (
        <div className="flex flex-col gap-1.5 text-[14px]">
          <div>
            <span className="label mr-2">Who can sign up</span>
            <span>{signup.summary}</span>
          </div>
          {signup.mode === "invite" && (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              {signup.allowlist.length ? `Allowed: ${signup.allowlist.join(", ")}` : "No emails are on the allowlist yet."}
            </p>
          )}
          <details className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
            <summary className="cursor-pointer select-none">How to change who can sign up</summary>
            <p className="mt-1 max-w-[68ch]">
              This comes from the server&apos;s settings: <code className="font-mono">SIGNUP</code> (open, invite or closed),{" "}
              <code className="font-mono">SIGNUP_ALLOWLIST</code> and <code className="font-mono">EUNOMIA_ADMIN_EMAILS</code>. Change
              them in your <code className="font-mono">.env</code> file and restart Eunomia.
            </p>
          </details>
        </div>
      )}

      {actionError && <ErrorLine error={actionError} />}

      <div className="overflow-x-auto">
        <table className="data-table w-full text-[13px]">
          <thead>
            <tr>
              <th className="text-left">Email</th>
              <th className="text-left w-[90px]">Role</th>
              <th className="text-left w-[120px] hidden md:table-cell">Joined</th>
              <th className="text-left w-[100px]">Last active</th>
              <th className="text-right w-[70px] hidden md:table-cell">Spaces</th>
              <th className="text-right">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {people === null ? (
              <TableSkeleton cols={6} />
            ) : people.length === 0 ? (
              <tr>
                <td colSpan={6} className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
                  No accounts yet.
                </td>
              </tr>
            ) : (
              people.map((p) => {
                const suspended = !!p.suspended_at;
                return (
                  <tr key={p.id} data-testid={`person-${p.email}`}>
                    <td className="max-w-[300px]">
                      <span className="flex flex-col min-w-0">
                        <span className="truncate" title={p.email} style={suspended ? { color: "var(--ink-dim)" } : undefined}>
                          {p.email}
                          {p.is_you && (
                            <span className="ml-1.5" style={{ color: "var(--ink-faint)" }}>
                              (you)
                            </span>
                          )}
                        </span>
                        {suspended && (
                          <span
                            className="text-[12px] inline-flex items-center gap-1.5"
                            style={{ color: "var(--critical)" }}
                            title={`Suspended ${new Date(p.suspended_at!).toLocaleString()}`}
                          >
                            <span className="dot" style={{ background: "var(--critical)" }} aria-hidden />
                            Suspended
                          </span>
                        )}
                      </span>
                    </td>
                    <td className="whitespace-nowrap">{p.role === "admin" ? "Admin" : "Member"}</td>
                    <td className="whitespace-nowrap hidden md:table-cell tabular-nums" style={{ color: "var(--ink-dim)" }}>
                      {date(p.created_at)}
                    </td>
                    <td
                      className="whitespace-nowrap tabular-nums"
                      style={{ color: "var(--ink-dim)" }}
                      title={p.last_active ? new Date(p.last_active).toLocaleString() : undefined}
                    >
                      {relativeTime(p.last_active ?? null)}
                    </td>
                    <td className="text-right tabular-nums hidden md:table-cell">{p.shared_spaces}</td>
                    <td className="text-right whitespace-nowrap">
                      <RowActions p={p} onError={setActionError} />
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
      <p className="text-[12px] max-w-[68ch]" style={{ color: "var(--ink-faint)" }}>
        Admins can see this page, the audit log and the server settings. A suspended account can&apos;t sign in, and its API tokens
        and connected apps stop working until you restore it.
      </p>
    </div>
  );
}
