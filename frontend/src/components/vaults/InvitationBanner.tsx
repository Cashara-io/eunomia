"use client";

// Invitations waiting for you, as a compact banner for Home: who invited you to which space, with Join and
// Decline. Renders nothing when there are none. After joining it says so and links to the space's people & topics.
import { useState } from "react";
import { Check, Mail } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useAcceptInvitation, useDeclineInvitation, useInvitations, useRefreshVaults } from "@/lib/queries/vaults";
import { ICON, JoinedNotice, inviterWords } from "./bits";

export default function InvitationBanner({ className = "" }: { className?: string } = {}) {
  const invitations = useInvitations().data ?? [];
  const accept = useAcceptInvitation();
  const decline = useDeclineInvitation();
  const refresh = useRefreshVaults();
  const [busyId, setBusyId] = useState<string | null>(null);
  const [joined, setJoined] = useState<{ id: string; name: string } | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  async function answer(id: string, name: string, join: boolean) {
    setBusyId(id);
    setError(null);
    try {
      if (join) {
        await accept.mutateAsync(id);
        setJoined({ id, name });
        await refresh();
      } else {
        await decline.mutateAsync(id);
      }
    } catch (e) {
      setError(
        failure(e, join ? "Could not join this space. The invitation may have been withdrawn." : "Could not decline this invitation. Try again."),
      );
    } finally {
      setBusyId(null);
    }
  }

  if (invitations.length === 0 && !joined && !error) return null;

  return (
    <section aria-label="Invitations" className={`flex flex-col ${className}`}>
      {invitations.map((inv) => (
        <div
          key={inv.vault_id}
          className="flex flex-wrap items-center gap-x-3 gap-y-2 py-2.5"
          style={{ borderTop: "1px solid var(--border)" }}
        >
          <Mail {...ICON} aria-hidden className="shrink-0" style={{ color: "var(--ink-dim)" }} />
          <p className="flex-1 min-w-[180px] text-[14px]">
            {inviterWords(inv.invited_by)} invited you to <span className="font-medium">{inv.vault_name}</span>
          </p>
          <div className="flex items-center gap-1.5 shrink-0">
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busyId === inv.vault_id}
              aria-label={`Decline invitation to ${inv.vault_name}`}
              onClick={() => answer(inv.vault_id, inv.vault_name, false)}
            >
              Decline
            </button>
            <button
              type="button"
              className="btn btn-primary btn-sm"
              disabled={busyId === inv.vault_id}
              aria-label={`Join ${inv.vault_name}`}
              onClick={() => answer(inv.vault_id, inv.vault_name, true)}
            >
              <Check {...ICON} aria-hidden />
              {busyId === inv.vault_id && accept.isPending ? "Joining…" : "Join"}
            </button>
          </div>
        </div>
      ))}
      {joined && <JoinedNotice id={joined.id} name={joined.name} onDismiss={() => setJoined(null)} />}
      {error && <ErrorLine error={error} />}
    </section>
  );
}
