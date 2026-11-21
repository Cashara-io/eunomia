"use client";

import { useState } from "react";
import { XIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Tooltip from "@/components/bits/Tooltip";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import type { Person } from "@/lib/gen";
import { usePersonApps, useRevokeAccess, useRevokePersonApp, useSetPersonRole, useSetSuspended } from "@/lib/queries/admin";
import { ICON, QUIET_DANGER, RevokeButton, relativeTime } from "./shared";

/* What each confirmation says: suspending and revoking sign them out, a role gives or takes the keys. */
const CONFIRM = {
  suspend: { note: "Signs them out everywhere", go: "Suspend", going: "Suspending…", danger: true },
  revoke: { note: "Ends every session, API token and connected app. They can sign in again.", go: "Revoke all access", going: "Revoking…", danger: true },
  admin: { note: "Can invite, suspend members, read the audit log and run access reviews.", go: "Make admin", going: "Saving…", danger: false },
} as const;

const ROLE_WORDS: Record<string, string> = { owner: "Owner", admin: "Admin", member: "Member" };
export const orgRoleLabel = (p: Pick<Person, "org_role">) => ROLE_WORDS[p.org_role] ?? "Member";

function Locked({ label, reason }: { label: string; reason: string }) {
  return (
    <Tooltip label={reason}>
      <span tabIndex={0} aria-label={`${label}: ${reason}`} className="inline-flex px-2 text-[13px] whitespace-nowrap" style={{ color: "var(--ink-faint)" }}>
        {label}
      </span>
    </Tooltip>
  );
}

/** Why a row has no actions for the viewer, for the tooltip. */
function why(p: Person): string {
  if (p.is_you) return "You can't change your own account here. Ask another admin.";
  if (p.org_role === "owner") return "This is the organisation's owner. Nobody changes it here.";
  if (p.admin) return "This account is an operator of the server. Only the server's operator changes it.";
  if (p.org_role === "admin") return "Only the organisation's owner manages an admin.";
  return "You can't change this account.";
}

/** Row actions for an owner or admin of the organisation: suspend or restore, make admin or take it back (the
 * owner), and end a member's access. Each shows only when the server says the viewer may (`can_*` on the row). */
export function OrgRowActions({ p, onError, onNotice }: { p: Person; onError: (f: Failure | null) => void; onNotice: (s: string | null) => void }) {
  const setSuspended = useSetSuspended();
  const setRole = useSetPersonRole();
  const revoke = useRevokeAccess();
  const [confirming, setConfirming] = useState<keyof typeof CONFIRM | null>(null);
  const busy = setSuspended.isPending || setRole.isPending || revoke.isPending;
  const suspended = !!p.suspended_at;
  const isAdmin = p.org_role === "admin";

  async function run(action: () => Promise<unknown>, fallback: string, done?: string) {
    onError(null);
    onNotice(null);
    try {
      await action();
      if (done) onNotice(done);
    } catch (e) {
      onError(failure(e, fallback));
    } finally {
      setConfirming(null);
    }
  }

  const doSuspend = () =>
    run(() => setSuspended.mutateAsync({ user_id: p.id, suspended: !suspended }), suspended ? `Could not restore ${p.email}.` : `Could not suspend ${p.email}.`);
  const doRole = () =>
    run(() => setRole.mutateAsync({ user_id: p.id, role: isAdmin ? "member" : "admin" }), isAdmin ? `Could not remove admin from ${p.email}.` : `Could not make ${p.email} an admin.`);
  const doRevoke = () =>
    run(
      async () => {
        const r = await revoke.mutateAsync(p.id);
        onNotice(`Ended ${r.sessions} ${r.sessions === 1 ? "session" : "sessions"}, ${r.tokens} ${r.tokens === 1 ? "token" : "tokens"} and ${r.apps} ${r.apps === 1 ? "app" : "apps"} of ${p.email}.`);
      },
      `Could not revoke ${p.email}'s access.`
    );

  if (confirming) {
    const c = CONFIRM[confirming];
    const go = confirming === "suspend" ? doSuspend : confirming === "revoke" ? doRevoke : doRole;
    return (
      <span className="inline-flex flex-wrap items-center justify-start sm:justify-end gap-1">
        <span className="text-[13px] mr-1 whitespace-normal" style={{ color: "var(--ink-dim)" }}>
          {c.note}
        </span>
        <button type="button" className={c.danger ? "btn btn-danger btn-sm" : "btn btn-sm"} disabled={busy} onClick={go}>
          {busy ? c.going : c.go}
        </button>
        <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label="Cancel" onClick={() => setConfirming(null)}>
          <XIcon {...ICON} />
        </button>
      </span>
    );
  }

  if (!p.can_suspend && !p.can_set_role && !p.can_revoke) {
    return <Locked label={p.is_you ? "That's you" : "No actions"} reason={why(p)} />;
  }

  return (
    <span className="inline-flex flex-wrap items-center justify-start sm:justify-end gap-1">
      {p.can_set_role &&
        !suspended &&
        (isAdmin ? (
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={doRole}>
            Remove admin
          </button>
        ) : (
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setConfirming("admin")}>
            Make admin
          </button>
        ))}
      {p.can_revoke && (
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} aria-label={`Revoke all access of ${p.email}`} onClick={() => setConfirming("revoke")}>
          Revoke access
        </button>
      )}
      {p.can_suspend &&
        (suspended ? (
          <button type="button" className="btn btn-sm" disabled={busy} onClick={doSuspend}>
            Restore
          </button>
        ) : (
          <button type="button" className={QUIET_DANGER} disabled={busy} onClick={() => setConfirming("suspend")}>
            Suspend
          </button>
        ))}
    </span>
  );
}

/** The apps a member connected, opened under their row next to their tokens: name, last used, Revoke. */
export function PersonApps({ p, onError }: { p: Person; onError: (f: Failure | null) => void }) {
  const query = usePersonApps(p.id);
  const revoke = useRevokePersonApp();
  if (query.isError) return <ErrorLine error={failure(query.error, `Could not load ${p.email}'s connected apps.`)} />;
  if (!query.data) return <LoadingSkeleton label="Loading apps" rows={1} />;
  if (query.data.length === 0) return null;
  return (
    <ul className="hairline-rows text-[13px]" aria-label={`${p.email}'s connected apps`}>
      {query.data.map((a) => (
        <li key={a.id} className="py-2 flex flex-wrap items-center gap-x-3 gap-y-0.5 min-w-0">
          <span className="flex flex-col min-w-0 flex-1">
            <span className="truncate font-medium" title={a.name}>
              {a.name} <span style={{ color: "var(--ink-faint)" }}>(connected app)</span>
            </span>
            <span className="text-[12px] tabular-nums" style={{ color: "var(--ink-dim)" }}>
              {`last used ${relativeTime(a.last_used_at ?? null)}`}
            </span>
          </span>
          <RevokeButton
            label={a.name}
            onRevoke={async () => {
              onError(null);
              try {
                await revoke.mutateAsync({ user_id: p.id, grant_id: a.id });
              } catch (e) {
                onError(failure(e, `Could not disconnect ${a.name}. Reload and try again.`));
              }
            }}
          />
        </li>
      ))}
    </ul>
  );
}
