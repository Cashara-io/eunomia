"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

import { useState } from "react";
import { XIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Tooltip from "@/components/bits/Tooltip";
import CopyButton from "@/components/bits/CopyButton";
import { usePeople, usePersonTokens, useRevokePersonToken, useSetAdmin, useSetSuspended } from "@/lib/queries/admin";
import type { Person } from "@/lib/gen";
import { useErasePerson, useErasures, useMakeWorkspaceOwner, useReapplyErasures, useSetLegalHold, useStartPersonExport } from "@/lib/queries/subject";
import { useMe } from "@/lib/queries/auth";
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

function RowActions({ p, onError, onData }: { p: Person; onError: (f: Failure | null) => void; onData: () => void }) {
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
          <XIcon {...ICON} />
        </button>
      </span>
    );
  }

  // your own row: one quiet note instead of two locked actions
  if (p.is_you) return <Locked label="That's you" reason="You can't change your own admin rights or suspend yourself. Ask another admin." />;

  return (
    <span className="inline-flex flex-wrap items-center justify-start sm:justify-end gap-1">
      <button type="button" className="btn btn-ghost btn-sm" aria-label={`Data of ${p.email}`} onClick={onData}>
        Data
      </button>
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

/* Someone's data, opened from "Data": export it for them, hold it, or delete the account and data (typing their email). */
function PersonData({ p, onError }: { p: Person; onError: (f: Failure | null) => void }) {
  const exp = useStartPersonExport();
  const hold = useSetLegalHold();
  const erase = useErasePerson();
  const owner = useMakeWorkspaceOwner();
  // the admin confirms with their own password; an admin with single sign-on only types the person's email
  const adminHasPassword = useMe().data?.has_password ?? true;
  const [confirming, setConfirming] = useState(false);
  const [typed, setTyped] = useState("");
  const [done, setDone] = useState<string | null>(null);
  const held = !!p.legal_hold_at;

  async function run(action: () => Promise<unknown>, fallback: string, ok: string) {
    onError(null);
    setDone(null);
    try {
      await action();
      setDone(ok);
    } catch (e) {
      onError(failure(e, fallback));
    }
  }

  return (
    <div className="py-2 flex flex-col gap-2 text-[13px]" data-testid={`person-data-${p.email}`}>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className="btn btn-sm"
          disabled={exp.isPending}
          onClick={() =>
            run(
              () => exp.mutateAsync(p.id),
              `Could not start an export for ${p.email}.`,
              "Export started. It is delivered to them in their Settings > Data, and their activity shows you started it. You see its status in your Settings > Data."
            )
          }
        >
          Export their data
        </button>
        <button
          type="button"
          className="btn btn-sm"
          disabled={hold.isPending}
          onClick={async () => {
            onError(null);
            setDone(null);
            try {
              const r = await hold.mutateAsync({ user_id: p.id, hold: !held });
              setDone(
                held
                  ? r.erasure_open
                    ? "Legal hold lifted. The deletion that was waiting carries on."
                    : "Legal hold lifted."
                  : r.erasure_open
                    ? "Legal hold placed. A deletion was already queued: it stops before its next step and waits until you lift the hold."
                    : "Legal hold placed. The account can't be erased until you lift it."
              );
            } catch (e) {
              onError(failure(e, held ? `Could not lift the hold on ${p.email}.` : `Could not place a hold on ${p.email}.`));
            }
          }}
        >
          {held ? "Lift legal hold" : "Place legal hold"}
        </button>
        {!p.workspace_owner && !p.is_you && (
          <button
            type="button"
            className="btn btn-sm"
            disabled={owner.isPending}
            title="A workspace has one owner. Hand it over before deleting the current owner's account."
            onClick={() => run(() => owner.mutateAsync(p.id), `Could not make ${p.email} the workspace owner.`, `${p.email} owns the workspace now.`)}
          >
            Make workspace owner
          </button>
        )}
        {!confirming && !p.is_you && (
          <button type="button" className={QUIET_DANGER} disabled={held} title={held ? "Lift the legal hold first." : undefined} onClick={() => setConfirming(true)}>
            Delete account and data
          </button>
        )}
      </div>
      {confirming && (
        <form
          className="flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void run(
              () => erase.mutateAsync(adminHasPassword ? { user_id: p.id, password: typed } : { user_id: p.id, confirm_email: typed }),
              `${p.email} was not deleted.`,
              "Deletion started. They are signed out, and their data is removed in the next few minutes."
            ).then(() => setConfirming(false));
          }}
        >
          <span style={{ color: "var(--ink-dim)" }}>
            Signs them out and deletes their account and everything that is theirs alone. What they wrote in shared spaces stays,
            with them removed as its author.{" "}
            {adminHasPassword ? "Type your own password to confirm." : <>Type <span className="font-mono">{p.email}</span> to confirm.</>}
          </span>
          <span className="flex flex-wrap items-center gap-2">
            <input
              className="field h-8 px-3 min-w-[16rem]"
              type={adminHasPassword ? "password" : "text"}
              autoComplete={adminHasPassword ? "current-password" : "off"}
              aria-label={adminHasPassword ? "Your password" : `Type ${p.email} to confirm`}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
            />
            <button type="submit" className="btn btn-danger-solid btn-sm" disabled={!typed || erase.isPending}>
              {erase.isPending ? "Deleting…" : "Delete"}
            </button>
            <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label="Cancel" onClick={() => setConfirming(false)}>
              <XIcon {...ICON} />
            </button>
          </span>
        </form>
      )}
      {done && (
        <span role="status" style={{ color: "var(--ink-dim)" }}>
          {done}
        </span>
      )}
    </div>
  );
}

const mentions = (r: { review?: unknown }) =>
  Object.values(((r.review as { mentions_left_in_others_content?: Record<string, number> } | undefined)?.mentions_left_in_others_content ?? {}) as Record<string, number>).reduce(
    (a, n) => a + (typeof n === "number" ? n : 0),
    0
  );

/* The proof each erasure leaves: when, how, whether the post-check found nothing. Never who it was. */
function Erasures() {
  const query = useErasures();
  const reapply = useReapplyErasures();
  const [error, setError] = useState<Failure | null>(null);
  const rows = query.data ?? [];
  return (
    <div className="flex flex-col gap-2" data-testid="erasure-records">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-reading font-semibold">Erasure records</h3>
        <button
          type="button"
          className="btn btn-ghost btn-sm"
          disabled={reapply.isPending}
          title="After restoring a backup: erase again anyone erased after the backup was taken. This also runs when the server starts."
          onClick={async () => {
            setError(null);
            try {
              await reapply.mutateAsync();
            } catch (e) {
              setError(failure(e, "Could not re-apply the erasures."));
            }
          }}
        >
          Re-apply after a restore
        </button>
      </div>
      {error && <ErrorLine error={error} />}
      {rows.length === 0 ? (
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          No accounts have been erased.
        </p>
      ) : (
        <ul className="hairline-rows text-[13px]">
          {rows.map((r) => {
            const left = Object.entries((r.post_check ?? {}) as Record<string, number>).filter(([, n]) => n > 0);
            return (
              <li key={r.id} className="py-2 flex flex-wrap items-center gap-x-3 gap-y-0.5">
                <span className="tabular-nums" style={{ color: "var(--ink-dim)" }}>
                  {new Date(r.completed_at ?? r.requested_at).toLocaleString()}
                </span>
                <span className="font-mono truncate max-w-[10rem]" title={`Reference ${r.subject_ref}`}>
                  {r.subject_ref.slice(0, 12)}
                </span>
                <span>{r.requested_by === "self" ? "by the person" : r.requested_by === "admin" ? "by an admin" : "re-applied after a restore"}</span>
                <span style={{ color: r.status === "done" ? "var(--good)" : r.status === "failed" ? "var(--critical)" : "var(--ink-dim)" }}>
                  {r.status === "done"
                    ? "Done, nothing left"
                    : r.status === "failed"
                      ? `Failed, retrying: still in ${left.map(([t]) => t).join(", ") || "unknown"}`
                      : r.status === "held"
                        ? "Waiting: a legal hold stopped it"
                        : "In progress"}
                </span>
                {mentions(r) > 0 && (
                  <span style={{ color: "var(--ink-dim)" }} title="Mentions of their email in content other people wrote. That content is theirs, so it was kept: review it.">
                    {mentions(r)} {mentions(r) === 1 ? "mention" : "mentions"} left in others&apos; content, for review
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
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
<LoadingSkeleton label="Loading tokens" rows={2} />
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
      {p.legal_hold_at && (
        <span className="text-[12px] inline-flex items-center gap-1.5" style={{ color: "var(--warning)" }} title={`Held since ${new Date(p.legal_hold_at).toLocaleString()}`}>
          <span className="dot" style={{ background: "var(--warning)" }} aria-hidden />
          Legal hold
        </span>
      )}
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
  // whose data panel is open under their row
  const [dataOf, setDataOf] = useState<string | null>(null);
  const toggleData = (id: string) => setDataOf((cur) => (cur === id ? null : id));

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
<li><LoadingSkeleton label="Loading people" /></li>
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
                <RowActions p={p} onError={setActionError} onData={() => toggleData(p.id)} />
              </div>
              {dataOf === p.id && <PersonData p={p} onError={setActionError} />}
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
              <th className="text-right w-[260px]">
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
                      <RowActions p={p} onError={setActionError} onData={() => toggleData(p.id)} />
                    </td>
                  </tr>,
                  tokensOf === p.id ? (
                    <tr key={`${p.id}-tokens`} data-testid={`person-tokens-${p.email}`}>
                      <td colSpan={7} className="h-auto! pl-8!">
                        <PersonTokens p={p} onError={setActionError} />
                      </td>
                    </tr>
                  ) : null,
                  dataOf === p.id ? (
                    <tr key={`${p.id}-data`}>
                      <td colSpan={7} className="h-auto! pl-8!">
                        <PersonData p={p} onError={setActionError} />
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
        and connected apps stop working until you restore it. Deleting an account removes everything that is theirs alone; a
        legal hold stops that until you lift it.
      </p>
      <Erasures />
    </div>
  );
}
