"use client";

import { ArrowsClockwiseIcon, ArrowsLeftRightIcon, ArrowsMergeIcon, CaretDownIcon, CaretRightIcon, CheckIcon, CopyIcon, CrownIcon, EnvelopeSimpleIcon, LockIcon, PencilSimpleIcon, PlusIcon, SignOutIcon, StackIcon, TrashIcon, UserPlusIcon, UsersIcon } from "@phosphor-icons/react/ssr";
import VaultChip, { VaultShape, vaultTone } from "@/components/VaultChip";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import PageHeader from "@/components/ui/PageHeader";
import { EmptyState } from "@/components/Shapes";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import StatusLabel from "@/components/ui/StatusLabel";
import AuthorTag from "@/components/AuthorTag";
import type { Vault, VaultMember, VaultRole } from "@/lib/types";
import { useMe } from "@/lib/queries/auth";
import { useConnectors, useRepointConnectors } from "@/lib/queries/connectors";
import {
  useAcceptInvitation,
  useCloneVault,
  useCreateVault,
  useDeclineInvitation,
  useDeleteVault,
  useInvitations,
  useInviteMember,
  useLeaveVault,
  useMergeVaults,
  useRefreshVaults,
  useRemoveMember,
  useRenameVault,
  useSetMemberRole,
  useTransferOwnership,
  useVaultMembers,
  useVaults,
} from "@/lib/queries/vaults";
import SyncMark from "@/components/bits/SyncMark";
import { spark } from "@/components/bits/Spark";
import Tooltip from "@/components/bits/Tooltip";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";
import TransferOwnershipPanel from "@/components/live/TransferOwnershipPanel";
import PeerSharing, { JoinSharedSpace } from "@/components/PeerSharing";
import { cssVar, prefersReducedMotion } from "@/components/bits/motion";

const ICON = { size: 18, weight: "duotone" } as const;
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function ago(iso: string | null): string | null {
  if (!iso) return null;
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

function RoleChip({ role }: { role: VaultRole }) {
  return (
    <span
      className="inline-flex items-center h-6 px-2 rounded-[4px] text-[12px] font-medium shrink-0"
      style={{
        border: "1px solid var(--border-strong)",
        color: role === "owner" ? "var(--ink)" : "var(--ink-dim)",
      }}
    >
      {role === "owner" ? "Owner" : "Member"}
    </span>
  );
}

/* The space's identity: its vault shape in its vault colour (a red circle
   for the personal space, a blue triangle for a shared space). Decorative;
   the name and the "Private to you"/"Shared space" words carry the meaning. */
function SpaceSwatch({ vault, size = 12 }: { vault: Vault; size?: number }) {
  return <VaultShape tone={vaultTone(vault.kind)} size={size} />;
}

/* A check that draws itself once on mount (the shared .bits-draw grammar, loaded by the bits imports above; reduced motion is handled there). */
function DrawnCheck() {
  return (
    <svg width={14} height={14} viewBox="0 0 16 16" fill="none" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden style={{ flexShrink: 0 }}>
      <path d="M3.5 8.4 6.6 11.3 12.5 4.9" stroke="var(--good)" pathLength={1} className="bits-draw" data-animate="" />
    </svg>
  );
}

/* New rows (since the previous load, never the first) settle in. A stable function, so React calls it once per mount. */
function arrive(el: HTMLElement | null) {
  if (!el || prefersReducedMotion()) return;
  el.animate([{ opacity: 0.001, transform: "translateY(-4px)" }, { opacity: 1, transform: "none" }], {
    duration: 200,
    easing: cssVar("--ease-out") || "ease-out",
  });
}

/* Overlapping identity tags, Figma-cursor style. Real member emails only. */
function MemberStack({ members }: { members: VaultMember[] | null }) {
  if (!members)
    return <span className="skeleton hidden sm:block h-5 w-20" aria-hidden />;
  const shown = members.slice(0, 3);
  const rest = members.length - shown.length;
  return (
    <span className="hidden sm:flex items-center gap-1 min-w-0" aria-hidden>
      {shown.map((m) => (
        <span key={m.email} className="max-w-[110px] min-w-0 flex">
          <AuthorTag name={m.email} />
        </span>
      ))}
      {rest > 0 && <span className="label font-mono">+{rest}</span>}
    </span>
  );
}

/* The inspector: everything you can do to one vault, Figma-style on the right. */
function VaultInspector({
  vault,
  me,
  inline = false,
  onVaultsChanged,
}: {
  vault: Vault;
  me: string | null;
  inline?: boolean;
  onVaultsChanged: () => void;
}) {
  const membersQuery = useVaultMembers(vault.id);
  const members = membersQuery.data ?? (membersQuery.isError ? [] : null);
  const inviteMember = useInviteMember();
  const removeMember = useRemoveMember();
  const changeRole = useSetMemberRole();
  const transfer = useTransferOwnership();
  const deleteVault = useDeleteVault();
  const leaveVault = useLeaveVault();
  const cloneVault = useCloneVault();
  const renameVault = useRenameVault();
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState(vault.name);
  const running = useRef(false);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<VaultRole>("member");
  const [touched, setTouched] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [handing, setHanding] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"leave" | "delete" | null>(null);
  const [cloning, setCloning] = useState(false);
  const [cloneName, setCloneName] = useState(`${vault.name} (copy)`);
  const [cloneBusy, setCloneBusy] = useState(false);
  const [sent, setSent] = useState<string | null>(null);
  const [cloned, setCloned] = useState(false);

  useEffect(() => {
    if (!cloned) return;
    const t = setTimeout(() => setCloned(false), 1200);
    return () => clearTimeout(t);
  }, [cloned]);

  const isOwner = vault.role === "owner";
  const isOrg = vault.kind !== "personal";
  const trimmed = email.trim();
  const invalid = touched && trimmed !== "" && !EMAIL_RE.test(trimmed);
  const already = members?.some(
    (m) => m.email.toLowerCase() === trimmed.toLowerCase(),
  );
  const created = ago(vault.created_at);
  // Last-owner rule: a shared space always needs an owner, so its only owner can't leave.
  const lastOwner = isOwner && isOrg && members !== null && members.filter((m) => m.role === "owner").length <= 1;

  async function invite() {
    setTouched(true);
    if (!trimmed || !EMAIL_RE.test(trimmed)) return;
    setBusy(true);
    setError(null);
    setSent(null);
    try {
      await inviteMember.mutateAsync({ id: vault.id, email: trimmed, role });
      setSent(
        `Invite sent. It waits on their Memory spaces page until they join or decline. They can't see anything here until they join.`,
      );
      setEmail("");
      setTouched(false);
    } catch (e) {
      setError(failure(e, "Could not send that invite.", " Check the address and try again."));
    } finally {
      setBusy(false);
    }
  }

  async function run(
    action: () => Promise<unknown>,
    fallback: string,
    after: () => void,
  ) {
    if (running.current) return; // a double click must not leave or delete twice
    running.current = true;
    setError(null);
    try {
      await action();
      after();
    } catch (e) {
      setError(failure(e, fallback));
    } finally {
      running.current = false;
    }
  }

  async function doRename() {
    const name = renameValue.trim();
    if (!name || name === vault.name) {
      setRenaming(false);
      return;
    }
    await run(() => renameVault.mutateAsync({ id: vault.id, name }), "Could not rename this space. Try a different name.", () => setRenaming(false));
  }

  async function doClone() {
    setCloneBusy(true);
    setError(null);
    try {
      await cloneVault.mutateAsync({ id: vault.id, name: cloneName.trim() || undefined });
      setCloning(false);
      setCloned(true);
      onVaultsChanged();
    } catch (e) {
      setError(failure(e, "Could not copy this space. Try a different name."));
    } finally {
      setCloneBusy(false);
    }
  }

  return (
    <aside
      className={inline ? "flex flex-col" : "panel flex flex-col"}
      style={
        inline ? { borderTop: "1px solid var(--border)", background: "var(--surface)" } : undefined
      }
      aria-label={`${vault.name} details`}
    >
      <div className="px-5 pt-4 pb-4 flex flex-col gap-1">
        {renaming ? (
          <form
            className="flex items-center gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              doRename();
            }}
          >
            <input
              aria-label="Space name"
              autoFocus
              maxLength={80}
              className="field h-10 px-3 text-[14px] flex-1 min-w-0"
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && setRenaming(false)}
            />
            <button type="submit" disabled={!renameValue.trim() || renameVault.isPending} className="btn btn-sm">
              {renameVault.isPending ? "Saving…" : "Save"}
            </button>
            <button type="button" onClick={() => setRenaming(false)} className="btn btn-ghost btn-sm">
              Cancel
            </button>
          </form>
        ) : (
          <div className="flex items-center gap-2.5 min-w-0">
            <SpaceSwatch vault={vault} size={16} />
            <h2 className="text-[22px] font-medium truncate tracking-[-0.03em]">
              {vault.name}
            </h2>
            <RoleChip role={vault.role} />
            {isOwner && (
              <Tooltip label="Rename space">
                <button
                  onClick={() => {
                    setRenameValue(vault.name);
                    setError(null);
                    setRenaming(true);
                  }}
                  aria-label="Rename space"
                  className="btn btn-ghost btn-sm btn-icon shrink-0"
                >
                  <PencilSimpleIcon {...ICON} />
                </button>
              </Tooltip>
            )}
          </div>
        )}
        <span className="label">
          {isOrg
            ? "Shared space · only the members listed below can read it"
            : "Personal space · private to you; nobody else can be added"}
          {created ? ` · created ${created}` : ""}
        </span>
      </div>

      <div
        className="px-5 pb-4 flex flex-col gap-2"
        style={{ borderTop: "1px solid var(--border)" }}
      >
        <h3 className="caps pt-4 flex items-center gap-1.5" style={{ color: "var(--ink-dim)" }}>
          Members
          {members && <span className="font-mono">{members.length}</span>}
        </h3>
        <ul className="hairline-rows">
          {members === null &&
            [0, 1].map((i) => (
              <li key={i} className="flex items-center gap-2 h-11">
                <span className="skeleton h-5 w-16" />
                <span className="skeleton h-4 flex-1" />
              </li>
            ))}
          {members?.map((m) => {
            const isMe =
              me !== null && m.email.toLowerCase() === me.toLowerCase();
            return (
              <li
                key={m.email}
                className="flex flex-wrap items-center gap-2 min-h-11 py-1.5"
              >
                <span className="flex max-w-[96px] min-w-0 shrink-0">
                  <AuthorTag name={m.email} />
                </span>
                <span
                  className="flex-1 min-w-0 truncate text-[13px]"
                  style={{ color: "var(--ink)" }}
                >
                  {m.email}
                  {isMe && (
                    <span style={{ color: "var(--ink-dim)" }}> (you)</span>
                  )}
                </span>
                <>
                    <RoleChip role={m.role} />
                    {isOwner && !isMe && (
                      <Tooltip label={m.role === "owner" ? `Make ${m.email} a member` : `Make ${m.email} an owner`}>
                        <button
                          onClick={() =>
                            run(
                              () => changeRole.mutateAsync({ id: vault.id, email: m.email, role: m.role === "owner" ? "member" : "owner" }),
                              "Could not change that role. Reload and try again.",
                              () => {},
                            )
                          }
                          aria-label={m.role === "owner" ? "Make member" : "Make owner"}
                          className="btn btn-ghost btn-sm btn-icon shrink-0"
                        >
                          <CrownIcon {...ICON} />
                        </button>
                      </Tooltip>
                    )}
                    {isOwner && !isMe && (
                      <Tooltip label={`Make ${m.email} the owner instead of you`}>
                        <button
                          onClick={() => setHanding(m.email)}
                          aria-label="Hand over space"
                          className="btn btn-ghost btn-sm btn-icon shrink-0"
                        >
                          <ArrowsLeftRightIcon {...ICON} />
                        </button>
                      </Tooltip>
                    )}
                    {isOwner && !isMe && (
                      <Tooltip label={`Remove ${m.email}`}>
                        <button
                          onClick={() => setRemoving(m.email)}
                          aria-label="Remove member"
                          className="btn btn-ghost btn-sm btn-icon shrink-0"
                          style={{ color: "var(--critical)" }}
                        >
                          <TrashIcon {...ICON} />
                        </button>
                      </Tooltip>
                    )}
                </>
              </li>
            );
          })}
        </ul>

        {isOwner && isOrg && (
          <form
            className="flex flex-col gap-1.5 pt-2"
            onSubmit={(e) => {
              e.preventDefault();
              invite();
            }}
            noValidate
          >
            <h3 className="section-title text-[15px]">Invite by email</h3>
            <label htmlFor={`invite-${vault.id}`} className="text-[13px] font-medium">
              Email address
            </label>
            <div className="flex flex-wrap items-center gap-2">
              <input
                id={`invite-${vault.id}`}
                type="email"
                className="field h-10 px-3 text-[14px] flex-1 min-w-[180px]"
                placeholder="person@example.com"
                value={email}
                aria-invalid={invalid || undefined}
                aria-describedby={`invite-help-${vault.id}`}
                onChange={(e) => {
                  setEmail(e.target.value);
                  setSent(null);
                }}
                onBlur={() => setTouched(true)}
                style={invalid ? { borderColor: "var(--critical)" } : undefined}
              />
              <Select
                className="h-10 text-[13px] w-32 shrink-0"
                value={role}
                aria-label="Role for invite"
                onChange={(v) => setRole(v as VaultRole)}
                options={[
                  { value: "member", label: "Member" },
                  { value: "owner", label: "Owner" },
                ]}
              />
            </div>
            <div className="flex items-start justify-between gap-2">
              <span
                id={`invite-help-${vault.id}`}
                className="text-[13px] pt-1.5"
                style={{
                  color: invalid ? "var(--critical)" : "var(--ink-dim)",
                }}
              >
                {invalid
                  ? "That isn't an email address. Use name@company.com."
                  : already
                    ? "Already a member."
                    : (sent ??
                      "An invitation gives no access until they accept it. Owners can invite and remove members; members can read and add memories.")}
              </span>
              <button
                type="submit"
                disabled={busy || !trimmed}
                className="btn btn-primary shrink-0"
              >
                {busy || sent ? <SyncMark status={busy ? "running" : "done"} /> : <UserPlusIcon {...ICON} />}
                {busy ? "Sending…" : "Send invite"}
              </button>
            </div>
          </form>
        )}
        {isOwner && <TransferOwnershipPanel vault={vault} members={members} me={me} />}
      </div>

      <div
        className="px-5 py-4 flex flex-col gap-2"
        style={{ borderTop: "1px solid var(--border)" }}
      >
        <PeerSharing vault={vault} />
        {cloning ? (
          <form
            className="flex flex-col gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              doClone();
            }}
          >
            <label htmlFor={`clone-${vault.id}`} className="text-[13px] font-medium">
              Name for the copy
            </label>
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              Copying creates a new space with the same memories and you as its only member. {vault.name} stays exactly as it is.
            </p>
            <input
              id={`clone-${vault.id}`}
              autoFocus
              className="field h-10 px-3 text-[14px]"
              value={cloneName}
              onChange={(e) => setCloneName(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && setCloning(false)}
            />
            <div className="flex justify-end gap-1.5">
              <button
                type="button"
                onClick={() => setCloning(false)}
                className="btn"
              >
                Cancel
              </button>
              <button type="submit" disabled={cloneBusy} className="btn btn-primary">
                {cloneBusy ? "Copying…" : "Create copy"}
              </button>
            </div>
          </form>
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <button
                onClick={() => {
                  setCloneName(`${vault.name} (copy)`);
                  setCloning(true);
                }}
                className="btn btn-sm"
              >
                {cloned ? <DrawnCheck /> : <CopyIcon {...ICON} />}
                Copy space
              </button>
              {isOrg && (
                <button
                  onClick={() => setConfirm("leave")}
                  disabled={lastOwner}
                  aria-describedby={lastOwner ? `last-owner-${vault.id}` : undefined}
                  className="btn btn-sm"
                >
                  <SignOutIcon {...ICON} />
                  Leave space…
                </button>
              )}
              {isOrg && isOwner && (
                <button onClick={() => setConfirm("delete")} className="btn btn-danger btn-sm">
                  <TrashIcon {...ICON} />
                  Delete space…
                </button>
              )}
            </div>
            {lastOwner && (
              <p id={`last-owner-${vault.id}`} className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                You are the only owner. Make another member an owner before leaving this space.
              </p>
            )}
          </>
        )}
        <ConfirmDialog
          open={confirm !== null}
          danger
          title={confirm === "delete" ? `Delete ${vault.name}?` : `Leave ${vault.name}?`}
          confirmLabel={confirm === "delete" ? `Delete ${vault.name}` : `Leave ${vault.name}`}
          busy={leaveVault.isPending || deleteVault.isPending}
          onCancel={() => setConfirm(null)}
          onConfirm={() =>
            confirm === "delete"
              ? run(
                  () => deleteVault.mutateAsync(vault.id),
                  "Could not delete this space. Reload and try again.",
                  () => {
                    setConfirm(null);
                    onVaultsChanged();
                  },
                )
              : run(
                  () => leaveVault.mutateAsync(vault.id),
                  "Could not leave this space. It needs an owner, so make someone else an owner first.",
                  () => {
                    setConfirm(null);
                    onVaultsChanged();
                  },
                )
          }
        >
          {confirm === "delete" ? (
            <p>
              Everyone in {vault.name} will lose access to this space and its saved memories, people and topics. This can&apos;t be
              undone. Copy the space first if you want to keep anything.
            </p>
          ) : (
            <p>
              You will lose access to the memories in this space. Other members keep access. To return, an owner will need to
              invite you again.
            </p>
          )}
        </ConfirmDialog>
        <ConfirmDialog
          open={removing !== null}
          danger
          title={`Remove ${removing ?? ""} from ${vault.name}?`}
          confirmLabel="Remove member"
          busy={removeMember.isPending}
          onCancel={() => setRemoving(null)}
          onConfirm={() =>
            removing &&
            run(
              () => removeMember.mutateAsync({ id: vault.id, email: removing }),
              "Could not remove that member. Reload and try again.",
              () => setRemoving(null),
            )
          }
        >
          <p>They lose access to this space and its memories straight away. What they added stays in the space. You can invite them again later.</p>
        </ConfirmDialog>
        <ConfirmDialog
          open={handing !== null}
          title={`Make ${handing ?? ""} the owner of ${vault.name}?`}
          confirmLabel="Hand over"
          busy={transfer.isPending}
          onCancel={() => setHanding(null)}
          onConfirm={() =>
            handing &&
            run(
              () => transfer.mutateAsync({ id: vault.id, email: handing }),
              "Could not hand over the space. Reload and try again.",
              () => setHanding(null),
            )
          }
        >
          <p>They become an owner and you become a member. Only an owner can make you an owner again.</p>
        </ConfirmDialog>
        {membersQuery.isError && <ErrorLine error={failure(membersQuery.error, "Could not load the members of this space.")} />}
        {error && <ErrorLine error={error} />}
      </div>
    </aside>
  );
}

function InvitationsPanel() {
  const invitationsQuery = useInvitations();
  const invitations = invitationsQuery.data ?? (invitationsQuery.isError ? [] : null);
  const acceptInvitation = useAcceptInvitation();
  const declineInvitation = useDeclineInvitation();
  const refreshVaults = useRefreshVaults();
  const [busyId, setBusyId] = useState<string | null>(null);
  const [joined, setJoined] = useState<string | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  async function accept(vaultId: string, button: HTMLElement, pointer: boolean) {
    setBusyId(vaultId);
    setError(null);
    try {
      await acceptInvitation.mutateAsync(vaultId);
      if (pointer) spark(button);
      // The dashed invite turns solid with a soft flash, then the vault row arrives in the list.
      setJoined(vaultId);
      if (!prefersReducedMotion()) await new Promise((r) => setTimeout(r, 180));
      await refreshVaults();
    } catch (e) {
      setError(
        failure(
          e,
          "Could not accept this invitation. It may have been withdrawn, so reload to check.",
        ),
      );
    } finally {
      setBusyId(null);
      setJoined(null);
    }
  }

  async function decline(vaultId: string) {
    setBusyId(vaultId);
    setError(null);
    try {
      await declineInvitation.mutateAsync(vaultId);
    } catch (e) {
      setError(
        failure(e, "Could not decline this invitation. Reload and try again."),
      );
    } finally {
      setBusyId(null);
    }
  }

  if (invitationsQuery.isError) return <ErrorLine error={failure(invitationsQuery.error, "Could not load your invitations.")} />;
  if (!invitations || invitations.length === 0) return null;

  return (
    <section className="flex flex-col gap-2.5" aria-labelledby="invites-title">
      <h2 id="invites-title" className="section-title flex items-center gap-2">
        <EnvelopeSimpleIcon {...ICON} aria-hidden />
        Pending invitations
        <span className="label font-mono">{invitations.length}</span>
      </h2>
      <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
        An invitation gives you no access until you join. Joining lets you read and add to that space.
      </p>
      {invitations.map((inv) => (
        <div
          key={inv.vault_id}
          className="ledger relative flex flex-wrap items-center gap-3 px-4 py-3"
          style={{ borderStyle: joined === inv.vault_id ? "solid" : "dashed", borderColor: "var(--border-strong)" }}
        >
          {joined === inv.vault_id && <JoinFlash />}
          <div className="flex-1 min-w-[160px]">
            <div className="text-[15px] font-medium min-w-0">
              <VaultChip tone={vaultTone(inv.vault_kind)} name={inv.vault_name} />
            </div>
            <div className="label mt-0.5">
              Invited as {inv.role === "owner" ? "an owner" : "a member"}
              {inv.vault_kind === "org" ? " of a shared space" : ""}
              {ago(inv.created_at) ? ` · ${ago(inv.created_at)}` : ""}
            </div>
          </div>
          <div className="flex items-center gap-1.5">
            <button
              onClick={() => decline(inv.vault_id)}
              disabled={busyId === inv.vault_id}
              aria-label="Decline invitation"
              className="btn btn-sm"
            >
              Decline
            </button>
            <button
              onClick={(e) => accept(inv.vault_id, e.currentTarget, e.detail > 0)}
              disabled={busyId === inv.vault_id}
              className="btn btn-primary btn-sm"
            >
              <CheckIcon {...ICON} />
              Join
            </button>
          </div>
        </div>
      ))}
      {error && <ErrorLine error={error} />}
    </section>
  );
}

function JoinFlash() {
  const ref = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    if (prefersReducedMotion()) return;
    ref.current?.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 180, easing: cssVar("--ease-out") || "ease-out" });
  }, []);
  return (
    <span
      ref={ref}
      aria-hidden
      className="absolute inset-0 pointer-events-none"
      style={{ background: "var(--accent-soft)", borderRadius: "inherit" }}
    />
  );
}

function VaultRow({
  vault,
  selected,
  onSelect,
}: {
  vault: Vault;
  selected: boolean;
  onSelect: () => void;
}) {
  const membersQuery = useVaultMembers(vault.id);
  const members = membersQuery.data ?? (membersQuery.isError ? [] : null);

  return (
    <button
      onClick={onSelect}
      aria-current={selected || undefined}
      className="w-full flex items-center gap-3 px-4 py-4 min-h-[64px] text-left transition-[background-color] duration-[120ms] hover:bg-[var(--surface-raised)]"
      style={selected ? { background: "var(--accent-soft)" } : undefined}
    >
      <span className="flex-1 min-w-0">
        <span className="flex items-center gap-2.5 min-w-0">
          <SpaceSwatch vault={vault} />
          <span className="text-[17px] font-medium truncate">
            {vault.name}
          </span>
          <RoleChip role={vault.role} />
        </span>
        <span className="label block mt-1 truncate">
          {vault.kind === "personal" ? "Private to you" : "Shared space"}
          {members
            ? ` · ${members.length} ${members.length === 1 ? "member" : "members"}`
            : ""}
        </span>
      </span>
      <MemberStack members={members} />
      <CaretRightIcon
        {...ICON}
        aria-hidden
        className="shrink-0"
        style={{ color: selected ? "var(--ink)" : "var(--ink-dim)" }}
      />
    </button>
  );
}

function CreateVaultForm({
  onCreated,
  onCancel,
}: {
  onCreated: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const createVault = useCreateVault();

  async function create() {
    const trimmed = name.trim();
    if (!trimmed) return;
    setBusy(true);
    setError(null);
    try {
      await createVault.mutateAsync({ name: trimmed });
      setName("");
      onCreated();
    } catch (e) {
      setError(
        failure(e, "Could not create that space. Try a different name."),
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="task-panel pop-in flex flex-col gap-2 px-5 py-4"
      style={{ transformOrigin: "top right" }}
      onSubmit={(e) => {
        e.preventDefault();
        create();
      }}
    >
      <label htmlFor="new-vault-name" className="text-[14px] font-medium">
        Name the space. You become its owner and can invite people next.
      </label>
      <div className="flex flex-wrap items-center gap-2">
        <input
          id="new-vault-name"
          autoFocus
          className="field h-10 px-3 text-[14px] flex-1 min-w-[180px]"
          placeholder="Space name, e.g. a project, client, team or household"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && onCancel()}
        />
        <button
          type="button"
          onClick={onCancel}
          className="btn"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={busy || !name.trim()}
          className="btn btn-primary"
        >
          {busy ? "Creating…" : "Create"}
        </button>
      </div>
      {error && <ErrorLine error={error} />}
    </form>
  );
}

const vaultLabel = (v: Vault) => (v.kind === "personal" ? "Personal" : v.name);

function MergeVaultsCard({
  vaults,
}: {
  vaults: Vault[];
}) {
  const mergeVaults = useMergeVaults();
  const [a, setA] = useState(vaults[0]?.id ?? "");
  const [b, setB] = useState(vaults[1]?.id ?? "");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [chips, setChips] = useState<{ from: [string, string]; to: string; id: number } | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const [open, setOpen] = useState(false);
  // the last merge, so connectors that filed into either vault can be pointed at the new one
  const [mergedInto, setMergedInto] = useState<{ from: string[]; id: string; name: string } | null>(null);
  const [repointed, setRepointed] = useState<{ ok: boolean; text: string } | null>(null);
  const connectors = useConnectors().data;
  const repoint = useRepointConnectors();
  const routedHere = (c: { routing?: { default_vault?: string; rules?: { vault: string }[] } }, ids: string[]) =>
    ids.includes(c.routing?.default_vault ?? "") || (c.routing?.rules ?? []).some((r) => ids.includes(r.vault));
  const affected = mergedInto && connectors ? connectors.filter((c) => routedHere(c, mergedInto.from)).length : 0;

  async function pointConnectors() {
    if (!mergedInto) return;
    setRepointed(null);
    try {
      const r = await repoint.mutateAsync({ from: mergedInto.from, to: mergedInto.id });
      setRepointed({ ok: true, text: `${r.connectors === 1 ? "1 connected source now saves" : `${r.connectors} connected sources now save`} into “${mergedInto.name}” from its next sync. Items already imported stay where they are until you move them from that source's page.` });
    } catch (e) {
      setRepointed({ ok: false, text: failure(e, "Could not point the connected sources at the new space.", " Try again.").message });
    }
  }

  async function merge() {
    setBusy(true);
    setError(null);
    setResult(null);
    setChips(null);
    setMergedInto(null);
    setRepointed(null);
    const label = (id: string) => vaults.find((v) => v.id === id);
    const from: [string, string] = [vaultLabel(label(a)!), vaultLabel(label(b)!)];
    try {
      const merged = await mergeVaults.mutateAsync({ a, b, name: name.trim() });
      setResult(`Created “${merged.name}” with ${merged.entities} entities.`);
      setChips({ from, to: merged.name, id: Date.now() });
      setMergedInto({ from: [a, b], id: merged.id, name: merged.name });
      setName("");
    } catch (e) {
      setError(
        failure(
          e,
          "Could not combine those spaces. Check you are a member of both and try again.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  const select = (value: string, set: (v: string) => void, label: string) => (
    <div className="flex flex-col gap-1.5 flex-1 min-w-[160px]">
      <span className="text-[13px] font-medium">{label}</span>
      <Select
        className="h-10 text-[14px] w-full"
        value={value}
        onChange={set}
        aria-label={label}
        options={vaults.map((v) => ({ value: v.id, label: vaultLabel(v) }))}
      />
    </div>
  );

  return (
    <section className="flex flex-col gap-3 items-start" aria-labelledby="merge-title">
      <button
        type="button"
        className="btn"
        aria-expanded={open}
        aria-controls="merge-panel"
        onClick={() => setOpen((o) => !o)}
        id="merge-title"
      >
        <ArrowsMergeIcon {...ICON} />
        Advanced: combine two spaces
        <CaretDownIcon {...ICON} aria-hidden style={{ transform: open ? "rotate(180deg)" : undefined }} />
      </button>
      {open && (
      <div id="merge-panel" className="ledger p-5 flex flex-col gap-4 w-full">
        <p className="text-[14px] max-w-[66ch]" style={{ color: "var(--ink-dim)" }}>
          Combining creates a brand new space with everything from both. The two originals stay exactly as they are,
          with the same members. Matching people and topics (same kind and name) are combined, and repeated facts are kept
          once. Only you are a member of the new space until you invite others.
        </p>
        <div className="flex items-end gap-2 flex-wrap">
          {select(a, setA, "First space")}
          <PlusIcon
            {...ICON}
            aria-hidden
            className="mb-3 hidden sm:block"
            style={{ color: "var(--ink-dim)" }}
          />
          {select(b, setB, "Second space")}
        </div>
        <div className="flex items-end gap-2 flex-wrap">
          <label className="flex flex-col gap-1.5 flex-1 min-w-[180px]">
            <span className="text-[13px] font-medium">Name for the new space</span>
            <input
              className="field h-10 px-3 text-[14px]"
              placeholder="New space name (optional)"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
          <button
            onClick={merge}
            disabled={busy || !a || a === b}
            className="btn btn-primary"
          >
            <ArrowsMergeIcon {...ICON} />
            {busy ? "Combining…" : "Merge into a new space"}
          </button>
        </div>
        {a === b && (
          <p className="label">Pick two different spaces to combine.</p>
        )}
        {result && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <p role="status" className="text-[14px]">
              <StatusLabel tone="good">{result}</StatusLabel>
            </p>
            {chips && <MergeChips key={chips.id} from={chips.from} to={chips.to} />}
          </div>
        )}
        {mergedInto && affected > 0 && !repointed?.ok && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <span className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
              {affected === 1 ? "A connected source still saves" : `${affected} connected sources still save`} into the spaces you combined. Pointing them at the new space only changes future syncs.
            </span>
            <button type="button" className="btn btn-sm" onClick={pointConnectors} disabled={repoint.isPending}>
              {repoint.isPending ? "Pointing…" : `Point ${affected === 1 ? "it" : "them"} at “${mergedInto.name}”`}
            </button>
          </div>
        )}
        {repointed && (
          <p role="status" className="text-[14px] fade-in" style={{ color: repointed.ok ? "var(--ink)" : "var(--critical)" }}>
            {repointed.text}
          </p>
        )}
        {error && <ErrorLine error={error} />}
      </div>
      )}
    </section>
  );
}

const CHIP =
  "inline-flex items-center h-5 px-1.5 rounded-[4px] text-[11.5px] font-medium whitespace-nowrap max-w-[140px] min-w-0";

/* After a merge: the two source chips glide together (260ms) and resolve into the new vault's name (180ms blur crossfade).
   Decorative and aria-hidden; the status line above already says it. The final state is the inline style, so reduced motion just shows it. */
function MergeChips({ from, to }: { from: [string, string]; to: string }) {
  const a = useRef<HTMLSpanElement>(null);
  const b = useRef<HTMLSpanElement>(null);
  const c = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const [A, B, C] = [a.current, b.current, c.current];
    if (!A || !B || !C || prefersReducedMotion()) return;
    const move = cssVar("--ease-in-out") || "ease-in-out";
    const out = cssVar("--ease-out") || "ease-out";
    const box = C.parentElement!.getBoundingClientRect();
    const mid = box.left + box.width / 2;
    const toMid = (el: HTMLElement) => {
      const r = el.getBoundingClientRect();
      return mid - (r.left + r.width / 2);
    };
    const glide = 260 / 440; // the glide's share of the 440ms timeline
    for (const el of [A, B]) {
      const d = `translateX(${toMid(el)}px)`;
      el.animate(
        [
          { transform: "none", opacity: 1, filter: "blur(0)", easing: move },
          { transform: d, opacity: 1, filter: "blur(0)", offset: glide, easing: out },
          { transform: d, opacity: 0, filter: "blur(2px)" },
        ],
        { duration: 440 },
      );
    }
    C.animate(
      [
        { opacity: 0, filter: "blur(2px)" },
        { opacity: 0, filter: "blur(2px)", offset: glide, easing: out },
        { opacity: 1, filter: "blur(0)" },
      ],
      { duration: 440 },
    );
  }, []);
  const quiet = { border: "var(--hair) solid var(--border-strong)", color: "var(--ink-dim)" };
  return (
    // Both sources stay in flow (invisible at rest) so the line never changes width.
    <span className="relative inline-flex items-center gap-1" aria-hidden>
      <span ref={a} className={CHIP} style={{ ...quiet, opacity: 0 }}>
        <span className="truncate">{from[0]}</span>
      </span>
      <span ref={b} className={CHIP} style={{ ...quiet, opacity: 0 }}>
        <span className="truncate">{from[1]}</span>
      </span>
      <span className="absolute inset-0 flex items-center justify-center">
        <span ref={c} className={CHIP} style={{ border: "var(--hair) solid var(--border-strong)", color: "var(--ink)" }}>
          <span className="truncate">{to}</span>
        </span>
      </span>
    </span>
  );
}

function SkeletonRows() {
  return (
    <div className="ledger hairline-rows" aria-hidden>
      {[0, 1].map((i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-4">
          <div className="flex-1 flex flex-col gap-1.5">
            <span className="skeleton h-4 w-40" />
            <span className="skeleton h-3 w-56 max-w-full" />
          </div>
          <span className="skeleton h-5 w-24" />
        </div>
      ))}
    </div>
  );
}

// The inspector floats on the right on wide screens and opens under its row on narrow ones.
const WIDE = "(min-width: 1024px)";
function useWide() {
  return useSyncExternalStore(
    (cb) => {
      const m = window.matchMedia(WIDE);
      m.addEventListener("change", cb);
      return () => m.removeEventListener("change", cb);
    },
    () => window.matchMedia(WIDE).matches,
    () => true,
  );
}

export default function VaultsPage() {
  const vaultsQuery = useVaults();
  const refetchAll = useRefreshVaults();
  const vaults = vaultsQuery.data ?? (vaultsQuery.isError ? [] : null);
  const me = useMe().data?.email ?? null;
  const [creating, setCreating] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Ids from the previous load, so rows that appear later (join, create, clone, merge) can settle in.
  const known = useRef<Set<string> | null>(null);
  const [fresh, setFresh] = useState<Set<string>>(new Set());

  const vaultList = vaultsQuery.data;
  useEffect(() => {
    if (!vaultList) return;
    const prev = known.current;
    if (prev) setFresh(new Set(vaultList.filter((v) => !prev.has(v.id)).map((v) => v.id)));
    known.current = new Set(vaultList.map((v) => v.id));
  }, [vaultList]);

  const personal = vaults?.filter((v) => v.kind === "personal") ?? [];
  const org = vaults?.filter((v) => v.kind !== "personal") ?? [];
  // Open on the first team vault: that is where the people are.
  const selected =
    vaults?.find((v) => v.id === selectedId) ?? org[0] ?? personal[0] ?? null;

  const wide = useWide();
  const inspectorFor = (inline: boolean) =>
    selected && (
      <VaultInspector
        key={selected.id}
        inline={inline}
        vault={selected}
        me={me}
        onVaultsChanged={() => setSelectedId(null)}
      />
    );

  const list = (items: Vault[]) => (
    <div className="ledger hairline-rows">
      {items.map((v) => (
        <div key={v.id} ref={fresh.has(v.id) ? arrive : undefined}>
          <VaultRow
            vault={v}
            selected={selected?.id === v.id}
            onSelect={() => setSelectedId(v.id)}
          />
          {!wide && selected?.id === v.id && inspectorFor(true)}
        </div>
      ))}
    </div>
  );

  return (
    <div className="max-w-6xl flex flex-col gap-7">
      <PageHeader
        icon={StackIcon} tone="project"
        title="Memory spaces"
        intro="Keep each project's memories in its own space. Your personal space is private; other spaces can be shared with people you invite. AI apps can use a space by its name."
      />

      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
          {vaults
            ? `${org.length} shared ${org.length === 1 ? "space" : "spaces"} · ${personal.length} personal`
            : "Loading your spaces…"}
        </p>
        <div className="flex items-center gap-2">
          <LiveStatusBadge />
          <button onClick={() => void refetchAll()} className="btn btn-icon" aria-label="Refresh spaces" title="Refresh">
            <ArrowsClockwiseIcon {...ICON} />
          </button>
          <JoinSharedSpace onJoined={(id) => setSelectedId(id)} />
          <button
            onClick={() => setCreating(true)}
            disabled={creating}
            className="btn btn-primary"
          >
            <PlusIcon {...ICON} />
            Create space
          </button>
        </div>
      </div>

      {vaultsQuery.isError && <ErrorLine error={failure(vaultsQuery.error, "Could not load your memory spaces.", " Reload the page to try again.")} />}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_380px] items-start">
        <div className="flex flex-col gap-9 min-w-0">
          <InvitationsPanel />

          <section
            className="flex flex-col gap-2.5"
            aria-labelledby="org-title"
          >
            <h2
              id="org-title"
              className="section-title flex items-center gap-2"
            >
              <UsersIcon
                {...ICON}
                aria-hidden
              />
              Shared spaces
              {vaults && <span className="label font-mono">{org.length}</span>}
            </h2>
            {creating && (
              <CreateVaultForm
                onCancel={() => setCreating(false)}
                onCreated={() => setCreating(false)}
              />
            )}
            {!vaults ? (
              <SkeletonRows />
            ) : org.length === 0 ? (
              !creating && (
                <EmptyState
                  variant="spaces"
                  title="No shared spaces yet"
                  action={
                    <button onClick={() => setCreating(true)} className="btn btn-sm">
                      <PlusIcon {...ICON} />
                      Create a space
                    </button>
                  }
                >
                  Create one for anything you want kept apart, such as a project or a team, or join one you were invited to.
                </EmptyState>
              )
            ) : (
              list(org)
            )}
          </section>

          <section
            className="flex flex-col gap-2.5"
            aria-labelledby="personal-title"
          >
            <h2
              id="personal-title"
              className="section-title flex items-center gap-2"
            >
              <LockIcon
                {...ICON}
                aria-hidden
              />
              Personal
            </h2>
            {!vaults ? <SkeletonRows /> : list(personal)}
          </section>

          {vaults && vaults.length > 1 && (
            <MergeVaultsCard vaults={vaults} />
          )}
        </div>

        {wide && (
          <div className="sticky top-8">
            {inspectorFor(false) ?? (
              <div className="panel p-5 flex flex-col gap-3" aria-hidden>
                <span className="skeleton h-4 w-24" />
                <span className="skeleton h-5 w-40" />
                <span className="skeleton h-24 w-full" />
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
