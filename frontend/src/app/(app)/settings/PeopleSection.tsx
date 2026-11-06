"use client";

import { useState } from "react";
import { X } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Tooltip from "@/components/bits/Tooltip";
import CopyButton from "@/components/bits/CopyButton";
import { usePeople, usePersonTokens, useRevokePersonToken, useSetAdmin, useSetSuspended } from "@/lib/queries/admin";
import type { Person } from "@/lib/gen";
import { ICON, PanelHead, QUIET_DANGER, RevokeButton, TableSkeleton, expiryLabel, relativeTime } from "./shared";

/* Why an admin can't be changed here, for the tooltip on the disabled button. */
const LOCKED: Record<string, string> = {
  creator: "This account set up the workspace, so it is always an admin.",
  operator: "The server's settings list this account as an admin. Change it there.",
};

const SIGNUP_DOCS = "https://github.com/Cashara-io/eunomia/blob/main/docs/deployment.md#2-env-vars-that-need-real-values";

/* What each confirmation says and does: suspending signs them out, making an admin hands over the keys. */
const CONFIRM = {
  suspend: { note: "Signs them out everywhere", go: "Suspend", going: "Suspending…", danger: true },
  admin: { note: "Can see everyone's account and change server settings.", go: "Make admin", going: "Saving…", danger: false },
} as const;

const date = (iso?: string | null) => (iso ? new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "—");

/* A disabled button with a reason: the tooltip sits on a wrapper because a disabled button gets no hover. */
function Locked({ label, reason }: { label: string; reason: string }) {
  return (
    // faint text, not a disabled button: it never stretches the row, and the tooltip says why
    <Tooltip label={reason}>
      <span tabIndex={0} aria-label={`${label}: ${reason}`} className="inline-flex px-2 text-[13px] whitespace-nowrap" style={{ color: "var(--ink-faint)" }}>
        {label}
      </span>
    </Tooltip>
  );
}

function RowActions({ p, onError }: { p: Person; onError: (f: Failure | null) => void }) {
  const setAdmin = useSetAdmin();
  const setSuspended = useSetSuspended();
  const [confirming, setConfirming] = useState<keyof typeof CONFIRM | null>(null);
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
      setConfirming(null);
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

  if (confirming) {
    const c = CONFIRM[confirming];
    return (
      <span className="inline-flex flex-wrap items-center justify-start sm:justify-end gap-1">
        <span className="text-[13px] mr-1 whitespace-normal" style={{ color: "var(--ink-dim)" }}>
          {c.note}
        </span>
        <button
          type="button"
          className={c.danger ? "btn btn-danger btn-sm" : "btn btn-sm"}
          disabled={busy}
          onClick={confirming === "suspend" ? toggleSuspended : toggleAdmin}
        >
          {busy ? c.going : c.go}
        </button>
        <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label="Cancel" onClick={() => setConfirming(null)}>
          <X {...ICON} />
        </button>
      </span>
    );
  }

  // your own row: one quiet note instead of two locked actions
  if (p.is_you) return <Locked label="That's you" reason="You can't change your own admin rights or suspend yourself. Ask another admin." />;

  return (
    <span className="inline-flex flex-wrap items-center justify-start sm:justify-end gap-1">
      {locked ? (
        <Locked label="Remove admin" reason={locked} />
      ) : p.is_you && p.role === "admin" ? (
        <Locked label="Remove admin" reason="You can't remove your own admin rights. Ask another admin." />
      ) : p.role === "admin" ? (
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={toggleAdmin}>
          Remove admin
        </button>
      ) : suspended ? null : (
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setConfirming("admin")}>
          Make admin
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
        <button type="button" className={QUIET_DANGER} disabled={busy} onClick={() => setConfirming("suspend")}>
          Suspend
        </button>
      )}
    </span>
  );
}

/* "2 tokens" as a toggle that opens the list under the row; plain "None" when there are none. */
function TokenCount({ p, open, onToggle }: { p: Person; open: boolean; onToggle: () => void }) {
  if (!p.tokens) return <span style={{ color: "var(--ink-faint)" }}>None</span>;
  return (
    <button
      type="button"
      className="underline decoration-[var(--border-strong)] underline-offset-2 hover:decoration-current tabular-nums"
      aria-expanded={open}
      aria-label={`${open ? "Hide" : "Show"} ${p.email}'s API tokens`}
      onClick={onToggle}
    >
      {p.tokens} {p.tokens === 1 ? "token" : "tokens"}
    </button>
  );
}

/* Someone's API tokens, opened from the count: name, created, last used, expires, and Revoke. */
function PersonTokens({ p, onError }: { p: Person; onError: (f: Failure | null) => void }) {
  const query = usePersonTokens(p.id);
  const revoke = useRevokePersonToken();
  const tokens = query.data;
  if (query.isError) return <ErrorLine error={failure(query.error, `Could not load ${p.email}'s tokens. Reload to try again.`)} />;
  if (!tokens)
    return (
      <p className="py-2 text-[13px]" style={{ color: "var(--ink-dim)" }}>
        Loading tokens…
      </p>
    );
  if (tokens.length === 0)
    return (
      <p className="py-2 text-[13px]" style={{ color: "var(--ink-dim)" }}>
        No API tokens left.
      </p>
    );
  return (
    <ul className="hairline-rows text-[13px]" aria-label={`${p.email}'s API tokens`}>
      {tokens.map((t) => {
        const exp = expiryLabel(t.expires_at ?? null);
        return (
          <li key={t.id} className="py-2 flex flex-wrap items-center gap-x-3 gap-y-0.5 min-w-0">
            <span className="flex flex-col min-w-0 flex-1">
              <span className="truncate font-medium" title={t.name}>
                {t.name}
              </span>
              <span className="text-[12px] tabular-nums" style={{ color: "var(--ink-dim)" }}>
                {[
                  `created ${date(t.created_at)}`,
                  `last used ${relativeTime(t.last_used_at ?? null)}`,
                  exp.expired ? "expired" : exp.text === "Never" ? "never expires" : `expires ${exp.text}`,
                ].join(" · ")}
              </span>
            </span>
            <RevokeButton
              label={t.name}
              onRevoke={async () => {
                onError(null);
                try {
                  await revoke.mutateAsync({ user_id: p.id, token_id: t.id });
                } catch (e) {
                  onError(failure(e, `Could not revoke ${t.name}. Reload and try again.`));
                }
              }}
            />
          </li>
        );
      })}
    </ul>
  );
}

/* The email (with "(you)") and, for a suspended account, a red "Suspended" line under it. */
function Email({ p }: { p: Person }) {
  const suspended = !!p.suspended_at;
  return (
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
  );
}

export function PeopleSection() {
  const query = usePeople();
  const data = query.data;
  const people = data?.people ?? null;
  const signup = data?.signup;
  const [actionError, setActionError] = useState<Failure | null>(null);
  // whose API tokens are open under their row
  const [tokensOf, setTokensOf] = useState<string | null>(null);
  const toggleTokens = (id: string) => setTokensOf((cur) => (cur === id ? null : id));

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="People">Everyone with an account in this workspace.</PanelHead>

      {query.isError && <ErrorLine error={failure(query.error, "Could not load the people in this workspace. Reload to try again.")} />}

      {signup && (
        <div className="flex flex-col gap-1.5 text-[14px]">
          {signup.mode !== "open" && (
            <div>
              <span className="label mr-2">Who can sign up</span>
              <span>{signup.summary}</span>
            </div>
          )}
          {signup.mode === "open" && (
            <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px]" data-testid="signup-open-warning">
              <span className="inline-flex items-center gap-1.5">
                <span className="dot" style={{ background: "var(--warning)" }} aria-hidden />
                Anyone who can reach this server can sign up. To require invites, set
              </span>
              <span className="inline-flex items-center gap-1">
                <code className="font-mono text-[13px]">SIGNUP=invite</code>
                <CopyButton value="SIGNUP=invite" label="Copy setting" size="sm" className="btn-ghost" />
              </span>
              <a href={SIGNUP_DOCS} target="_blank" rel="noopener noreferrer" className="underline" style={{ color: "var(--ink-dim)" }}>
                How to change this
              </a>
            </p>
          )}
          {signup.mode !== "open" && (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              {signup.mode === "invite" && (signup.allowlist.length ? `Allowed: ${signup.allowlist.join(", ")}. ` : "No emails are on the allowlist yet. ")}
              <a href={SIGNUP_DOCS} target="_blank" rel="noopener noreferrer" className="underline">
                How to change this
              </a>
            </p>
          )}
        </div>
      )}

      {actionError && <ErrorLine error={actionError} />}

      {/* Phones: one stacked entry per person (email, then the facts, then the actions), no sideways scroll. */}
      <ul className="sm:hidden hairline-rows text-[13px]" aria-label="People">
        {people === null ? (
          <li className="py-3" style={{ color: "var(--ink-dim)" }}>
            Loading…
          </li>
        ) : people.length === 0 ? (
          <li className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
            No accounts yet.
          </li>
        ) : (
          people.map((p) => (
            <li key={p.id} className="py-3 flex flex-col gap-1 min-w-0" data-testid={`person-card-${p.email}`}>
              <Email p={p} />
              <span className="text-[12px] tabular-nums" style={{ color: "var(--ink-dim)" }}>
                {[
                  p.role === "admin" ? "Admin" : "Member",
                  `joined ${date(p.created_at)}`,
                  `active ${relativeTime(p.last_active ?? null)}`,
                  `${p.shared_spaces} ${p.shared_spaces === 1 ? "space" : "spaces"}`,
                ].join(" · ")}
                {p.tokens > 0 && (
                  <>
                    {" · "}
                    <TokenCount p={p} open={tokensOf === p.id} onToggle={() => toggleTokens(p.id)} />
                  </>
                )}
              </span>
              {tokensOf === p.id && (
                <div className="pl-3 border-l" style={{ borderColor: "var(--border)" }}>
                  <PersonTokens p={p} onError={setActionError} />
                </div>
              )}
              {/* pulled left by the buttons' own padding, so the actions line up with the email above */}
              <div className={`pt-1 ${p.is_you ? "" : "-ml-2.5"}`}>
                <RowActions p={p} onError={setActionError} />
              </div>
            </li>
          ))
        )}
      </ul>

      <div className="hidden sm:block overflow-x-auto">
        <table className="data-table w-full table-fixed text-[13px]">
          <thead>
            <tr>
              <th className="text-left">Email</th>
              <th className="text-left w-[80px]">Role</th>
              <th className="text-left w-[110px] hidden lg:table-cell">Joined</th>
              <th className="text-left w-[100px]">Last active</th>
              <th className="text-right w-[70px] hidden lg:table-cell">Spaces</th>
              <th className="text-left w-[90px]">API tokens</th>
              <th className="text-right w-[210px]">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {people === null ? (
              <TableSkeleton cols={7} />
            ) : people.length === 0 ? (
              <tr>
                <td colSpan={7} className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
                  No accounts yet.
                </td>
              </tr>
            ) : (
              people.flatMap((p) => [
                  <tr key={p.id} data-testid={`person-${p.email}`}>
                    <td>
                      <Email p={p} />
                    </td>
                    <td className="whitespace-nowrap">{p.role === "admin" ? "Admin" : "Member"}</td>
                    <td className="whitespace-nowrap hidden lg:table-cell tabular-nums" style={{ color: "var(--ink-dim)" }}>
                      {date(p.created_at)}
                    </td>
                    <td
                      className="whitespace-nowrap tabular-nums"
                      style={{ color: "var(--ink-dim)" }}
                      title={p.last_active ? new Date(p.last_active).toLocaleString() : undefined}
                    >
                      {relativeTime(p.last_active ?? null)}
                    </td>
                    <td className="text-right tabular-nums hidden lg:table-cell">{p.shared_spaces}</td>
                    <td className="whitespace-nowrap">
                      <TokenCount p={p} open={tokensOf === p.id} onToggle={() => toggleTokens(p.id)} />
                    </td>
                    <td className="text-right whitespace-nowrap">
                      <RowActions p={p} onError={setActionError} />
                    </td>
                  </tr>,
                  tokensOf === p.id ? (
                    <tr key={`${p.id}-tokens`} data-testid={`person-tokens-${p.email}`}>
                      <td colSpan={7} className="h-auto! pl-8!">
                        <PersonTokens p={p} onError={setActionError} />
                      </td>
                    </tr>
                  ) : null,
                ])
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
