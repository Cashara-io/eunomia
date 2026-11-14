"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import CopyButton from "@/components/bits/CopyButton";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import { useCreateInvitation, useOrgInvitations, useOrgSeats, useResendInvitation, useRevokeInvitation } from "@/lib/queries/org";
import type { OrgInvitation, OrgInviteRole } from "@/lib/gen";
import { RevokeButton } from "./shared";

/* The link is built from this page's own address, so it opens the same site the admin is on. The token
   sits after `#`, which the browser never sends to a server. */
const linkFor = (token: string) => `${window.location.origin}/invite#${token}`;

const when = (iso: string) => new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });

/* A link that was just made: shown once, with a copy button. Eunomia sends no email. */
function FreshLink({ email, link, onDone }: { email: string; link: string; onDone: () => void }) {
  return (
    <div className="flex flex-col gap-2 p-3 rounded-[10px]" style={{ background: "var(--surface-2, var(--bg-subtle))" }} data-testid="invite-link">
      <p className="text-[13px]">
        Send this link to <strong>{email}</strong> yourself. Eunomia doesn&apos;t send email. It works once and expires in 7 days.
        You won&apos;t see it again, but you can make a new one with Resend.
      </p>
      <div className="flex items-center gap-2 min-w-0">
        <code className="field flex-1 min-w-0 truncate px-3 py-2 text-[12px] font-mono" data-testid="invite-link-text">
          {link}
        </code>
        <CopyButton value={link} label="Copy link" size="sm" />
        <button type="button" className="btn btn-ghost btn-sm" onClick={onDone}>
          Done
        </button>
      </div>
    </div>
  );
}

function Row({ inv, onLink, onError }: { inv: OrgInvitation; onLink: (email: string, link: string) => void; onError: (f: Failure | null) => void }) {
  const resend = useResendInvitation();
  const revoke = useRevokeInvitation();
  return (
    <li className="py-2.5 flex flex-wrap items-center gap-x-3 gap-y-1 min-w-0" data-testid={`invitation-${inv.email}`}>
      <span className="flex flex-col min-w-0 flex-1">
        <span className="truncate" title={inv.email}>
          {inv.email}
        </span>
        <span className="text-[12px] tabular-nums" style={{ color: inv.expired ? "var(--critical)" : "var(--ink-dim)" }}>
          {[inv.role === "admin" ? "Admin" : "Member", inv.expired ? `expired ${when(inv.expires_at)}` : `expires ${when(inv.expires_at)}`, inv.invited_by && `sent by ${inv.invited_by}`]
            .filter(Boolean)
            .join(" · ")}
        </span>
      </span>
      <button
        type="button"
        className="btn btn-ghost btn-sm"
        disabled={resend.isPending}
        onClick={async () => {
          onError(null);
          try {
            const r = await resend.mutateAsync(inv.id);
            onLink(r.invitation.email, linkFor(r.token));
          } catch (e) {
            onError(failure(e, `Could not make a new link for ${inv.email}.`));
          }
        }}
      >
        {resend.isPending ? "Making a link…" : "Resend"}
      </button>
      <RevokeButton
        label={`the invitation for ${inv.email}`}
        onRevoke={async () => {
          onError(null);
          try {
            await revoke.mutateAsync(inv.id);
          } catch (e) {
            onError(failure(e, `Could not withdraw the invitation for ${inv.email}.`));
          }
        }}
      />
    </li>
  );
}

/** Settings > People: invite by email, see open invitations and the organisation's seats. */
export function InvitationsSection() {
  const list = useOrgInvitations();
  const seats = useOrgSeats();
  const create = useCreateInvitation();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<OrgInviteRole>("member");
  const [error, setError] = useState<Failure | null>(null);
  const [fresh, setFresh] = useState<{ email: string; link: string } | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const r = await create.mutateAsync({ email, role });
      setFresh({ email: r.invitation.email, link: linkFor(r.token) });
      setEmail("");
    } catch (err) {
      setError(failure(err, "Could not create the invitation."));
    }
  }

  const s = seats.data;
  return (
    <section className="flex flex-col gap-3" aria-labelledby="invitations-title">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 id="invitations-title" className="text-[15px] font-medium">
          Invite people
        </h2>
        {s && (
          <span className="text-[13px] tabular-nums" style={{ color: "var(--ink-dim)" }} data-testid="org-seats">
            {s.active_members} active {s.active_members === 1 ? "member" : "members"} · {s.pending_invitations} pending
            {s.suspended_members > 0 ? ` · ${s.suspended_members} suspended` : ""}
          </span>
        )}
      </div>
      <form onSubmit={submit} className="flex flex-wrap items-end gap-2">
        <div className="flex flex-col gap-1 flex-1 min-w-[220px]">
          <label htmlFor="invite-email" className="label">
            Email
          </label>
          <input
            id="invite-email"
            type="email"
            required
            spellCheck={false}
            autoComplete="off"
            className="field h-10 px-3 text-[14px]"
            placeholder="colleague@company.com"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor="invite-role" className="label">
            Role
          </label>
          <select id="invite-role" className="field h-10 px-3 text-[14px]" value={role} onChange={(e) => setRole(e.target.value as OrgInviteRole)}>
            <option value="member">Member</option>
            <option value="admin">Admin (can invite people)</option>
          </select>
        </div>
        <button type="submit" className="btn btn-primary h-10" disabled={create.isPending}>
          {create.isPending ? "Inviting…" : "Invite"}
        </button>
      </form>
      {error && <ErrorLine error={error} />}
      {fresh && <FreshLink email={fresh.email} link={fresh.link} onDone={() => setFresh(null)} />}
      {list.isError && <ErrorLine error={failure(list.error, "Could not load the invitations. Reload to try again.")} />}
      {list.data === undefined && !list.isError ? (
        <LoadingSkeleton label="Loading invitations" rows={1} />
      ) : list.data && list.data.length > 0 ? (
        <ul className="hairline-rows text-[13px]" aria-label="Pending invitations">
          {list.data.map((inv) => (
            <Row key={inv.id} inv={inv} onLink={(email, link) => setFresh({ email, link })} onError={setError} />
          ))}
        </ul>
      ) : (
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          No pending invitations.
        </p>
      )}
    </section>
  );
}
