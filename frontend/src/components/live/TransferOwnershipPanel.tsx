"use client";

import { useState } from "react";
import { ArrowRightLeft } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useTransferOwnership } from "@/lib/queries/vaults";
import type { Vault, VaultMember } from "@/lib/types";

/**
 * "Hand over this space" (#134): a plain-language way for an owner to give a space to another member,
 * the same action as the `vault_transfer_ownership` tool. Shown to owners only.
 */
export default function TransferOwnershipPanel({ vault, members, me }: { vault: Vault; members: VaultMember[] | null; me: string | null }) {
  const transfer = useTransferOwnership();
  const [to, setTo] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const others = (members ?? []).filter((m) => !me || m.email.toLowerCase() !== me.toLowerCase());
  const target = others.find((m) => m.email === to)?.email ?? "";

  async function handOver() {
    setError(null);
    try {
      await transfer.mutateAsync({ id: vault.id, email: target });
      setDone(target);
      setConfirming(false);
      setTo("");
    } catch (e) {
      setError(failure(e, "Could not hand over this space.", " Reload the page and try again."));
    }
  }

  return (
    <section className="flex flex-col gap-2" aria-labelledby={`handover-${vault.id}`}>
      <h3 id={`handover-${vault.id}`} className="section-title flex items-center gap-2">
        <ArrowRightLeft size={16} strokeWidth={1.75} aria-hidden />
        Hand over this space
      </h3>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        Make another member the owner. You stay in the space as a member, but you can no longer rename it, delete it or change who
        is in it.
      </p>
      {members !== null && others.length === 0 ? (
        <p className="text-[13px]" style={{ color: "var(--ink-faint)" }}>
          No one else is in this space yet. Invite someone, wait for them to accept, and then you can hand it over.
        </p>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <Select
            className="h-10 text-[13px] flex-1 min-w-[180px]"
            aria-label="New owner"
            placeholder="Choose the new owner"
            value={to}
            disabled={members === null || transfer.isPending}
            onChange={(v) => {
              setTo(v);
              setConfirming(false);
              setDone(null);
            }}
            options={others.map((m) => ({ value: m.email, label: m.role === "owner" ? `${m.email} (already an owner)` : m.email }))}
          />
          {confirming ? (
            <>
              <button type="button" className="btn btn-primary" disabled={transfer.isPending} onClick={() => void handOver()}>
                {transfer.isPending ? "Handing over…" : "Yes, hand it over"}
              </button>
              <button type="button" className="btn btn-ghost" onClick={() => setConfirming(false)}>
                Cancel
              </button>
            </>
          ) : (
            <button type="button" className="btn" disabled={!target} onClick={() => setConfirming(true)}>
              Hand over
            </button>
          )}
        </div>
      )}
      {confirming && target && (
        <p className="text-[13px]" role="status">
          {target} will own &ldquo;{vault.name}&rdquo; and you will become a member. Only an owner can undo this.
        </p>
      )}
      {done && (
        <p className="text-[13px]" role="status" style={{ color: "var(--good)" }}>
          Done: {done} now owns this space.
        </p>
      )}
      {error && <ErrorLine error={error} />}
    </section>
  );
}
