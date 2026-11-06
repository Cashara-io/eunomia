"use client";

// Everything you can do with one memory space, in plain sections: who is in it, sharing it with
// another Eunomia (folded away), and the rarer actions (copy, hand over, leave, delete) at the bottom.
import { useEffect, useRef, useState } from "react";
import { CopyIcon, PencilSimpleIcon, SignOutIcon, SlidersHorizontalIcon, TrashIcon, UserPlusIcon, UsersIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import AuthorTag from "@/components/AuthorTag";
import SyncMark from "@/components/bits/SyncMark";
import Tooltip from "@/components/bits/Tooltip";
import TransferOwnershipPanel from "@/components/live/TransferOwnershipPanel";
import PeerSharing from "@/components/PeerSharing";
import type { Vault, VaultRole } from "@/lib/types";
import {
  useCloneVault,
  useDeleteVault,
  useInviteMember,
  useLeaveVault,
  useRemoveMember,
  useRenameVault,
  useSetMemberRole,
  useVaultMembers,
} from "@/lib/queries/vaults";
import { DetailSection, DrawnCheck, ICON, RoleChip, SpaceSwatch, ago, peopleLine, sameEmail } from "./bits";

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const ROLE_OPTIONS = [
  { value: "member", label: "Member" },
  { value: "owner", label: "Owner" },
];

export default function SpaceDetail({
  vault,
  me,
  onVaultsChanged,
}: {
  vault: Vault;
  me: string | null;
  onVaultsChanged: () => void;
}) {
  const membersQuery = useVaultMembers(vault.id);
  const members = membersQuery.data ?? (membersQuery.isError ? [] : null);
  const inviteMember = useInviteMember();
  const removeMember = useRemoveMember();
  const changeRole = useSetMemberRole();
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
  const already = members?.some((m) => sameEmail(m.email, trimmed));
  const created = ago(vault.created_at);
  const others = (members ?? []).filter((m) => !sameEmail(m.email, me));
  // Last-owner rule: a shared space always needs an owner, so its only owner can't leave.
  const lastOwner = isOwner && isOrg && members !== null && members.filter((m) => m.role === "owner").length <= 1;
  const canLeave = isOrg && members !== null && !lastOwner;
  const canHandOver = isOwner && isOrg && others.length > 0;

  async function invite() {
    setTouched(true);
    if (!trimmed || !EMAIL_RE.test(trimmed)) return;
    setBusy(true);
    setError(null);
    setSent(null);
    try {
      await inviteMember.mutateAsync({ id: vault.id, email: trimmed, role });
      setSent("Invite sent. They'll see it on their Memory spaces page, and can see nothing here until they join.");
      setEmail("");
      setTouched(false);
    } catch (e) {
      setError(failure(e, "Could not send that invite.", " Check the address and try again."));
    } finally {
      setBusy(false);
    }
  }

  async function run(action: () => Promise<unknown>, fallback: string, after: () => void) {
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

  const helpText = invalid
    ? "That isn't an email address. Use name@company.com."
    : already
      ? "They're already in this space."
      : (sent ??
        (role === "owner"
          ? "Owners can invite and remove people, rename the space and delete it."
          : "Members can read and add memories. Nothing is shared until they accept."));

  return (
    <article className="panel flex flex-col min-w-0" aria-label={`${vault.name} details`}>
      {/* Header: what this space is, at a glance. */}
      <header className="px-5 sm:px-6 pt-5 pb-5 flex flex-col gap-1.5">
        {renaming ? (
          <form
            className="flex flex-wrap items-center gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              doRename();
            }}
          >
            <input
              aria-label="Space name"
              autoFocus
              maxLength={80}
              className="field h-10 px-3 text-[14px] flex-1 min-w-[160px]"
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
            <h2 className="text-[22px] font-medium tracking-[-0.03em] truncate">{vault.name}</h2>
            {!isOwner && <RoleChip role={vault.role} />}
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
        <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
          {isOrg
            ? `${peopleLine(vault, members)}. Only the people listed here can see what's in it.`
            : "Only you can see this space. To work with other people, create a shared space."}
          {created ? ` Created ${created}.` : ""}
        </p>
      </header>

      {/* People */}
      {isOrg && (
        <DetailSection
          title="People"
          icon={<UsersIcon {...ICON} aria-hidden />}
          aside={members && <span className="label font-mono">{members.length}</span>}
        >
          <ul className="hairline-rows">
            {members === null &&
              [0, 1].map((i) => (
                <li key={i} className="flex items-center gap-2 h-11">
                  <span className="skeleton h-5 w-16" />
                  <span className="skeleton h-4 flex-1" />
                </li>
              ))}
            {members?.map((m) => {
              const isMe = sameEmail(m.email, me);
              return (
                <li key={m.email} className="flex flex-wrap items-center gap-x-3 gap-y-1.5 min-h-12 py-2">
                  <span className="flex max-w-[96px] min-w-0 shrink-0" aria-hidden>
                    <AuthorTag name={m.email} />
                  </span>
                  <span className="flex-1 min-w-[140px] truncate text-[14px]" style={{ color: "var(--ink)" }}>
                    {m.email}
                    {isMe && <span style={{ color: "var(--ink-dim)" }}> (you)</span>}
                  </span>
                  {isOwner && !isMe ? (
                    <span className="flex items-center gap-1.5 shrink-0">
                      <Select
                        className="h-9 text-[13px] w-[110px]"
                        value={m.role}
                        aria-label={`Role for ${m.email}`}
                        disabled={changeRole.isPending}
                        onChange={(v) =>
                          v !== m.role &&
                          run(
                            () => changeRole.mutateAsync({ id: vault.id, email: m.email, role: v as VaultRole }),
                            "Could not change that role. Reload and try again.",
                            () => {},
                          )
                        }
                        options={ROLE_OPTIONS}
                      />
                      <button
                        type="button"
                        onClick={() => setRemoving(m.email)}
                        aria-label={`Remove ${m.email}`}
                        className="btn btn-ghost btn-sm"
                        style={{ color: "var(--critical)" }}
                      >
                        Remove
                      </button>
                    </span>
                  ) : (
                    <RoleChip role={m.role} />
                  )}
                </li>
              );
            })}
          </ul>

          {isOwner && (
            <form
              className="flex flex-col gap-1.5 pt-2"
              onSubmit={(e) => {
                e.preventDefault();
                invite();
              }}
              noValidate
            >
              <label htmlFor={`invite-${vault.id}`} className="text-[14px] font-medium">
                Add someone by email
              </label>
              <div className="flex flex-wrap items-center gap-2">
                <input
                  id={`invite-${vault.id}`}
                  type="email"
                  className="field h-10 px-3 text-[14px] flex-1 min-w-[200px]"
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
                  className="h-10 text-[13px] w-[110px] shrink-0"
                  value={role}
                  aria-label="Role for invite"
                  onChange={(v) => setRole(v as VaultRole)}
                  options={ROLE_OPTIONS}
                />
                <button type="submit" disabled={busy || !trimmed} className="btn btn-primary shrink-0">
                  {busy || sent ? <SyncMark status={busy ? "running" : "done"} /> : <UserPlusIcon {...ICON} />}
                  {busy ? "Sending…" : "Send invite"}
                </button>
              </div>
              <p
                id={`invite-help-${vault.id}`}
                role={sent ? "status" : undefined}
                className="text-[13px]"
                style={{ color: invalid ? "var(--critical)" : "var(--ink-dim)" }}
              >
                {helpText}
              </p>
            </form>
          )}
          {membersQuery.isError && <ErrorLine error={failure(membersQuery.error, "Could not load the people in this space.")} />}
        </DetailSection>
      )}

      <PeerSharing vault={vault} />

      {/* The rarer actions, quiet at the bottom. */}
      <DetailSection title="More options" icon={<SlidersHorizontalIcon {...ICON} aria-hidden />}>
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
              The copy is a new space with the same memories, and only you in it. {vault.name} stays exactly as it is.
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
              <button type="button" onClick={() => setCloning(false)} className="btn">
                Cancel
              </button>
              <button type="submit" disabled={cloneBusy} className="btn btn-primary">
                {cloneBusy ? "Copying…" : "Create copy"}
              </button>
            </div>
          </form>
        ) : (
          <OptionRow
            title="Make a copy"
            body="A new space with the same memories. This one stays as it is."
            action={
              <button
                onClick={() => {
                  setCloneName(`${vault.name} (copy)`);
                  setCloning(true);
                }}
                className="btn btn-sm"
              >
                {cloned ? <DrawnCheck /> : <CopyIcon {...ICON} />}
                CopyIcon space
              </button>
            }
          />
        )}

        {canHandOver && (
          <div className="pt-3" style={{ borderTop: "1px solid var(--border)" }}>
            <TransferOwnershipPanel vault={vault} members={members} me={me} />
          </div>
        )}

        {canLeave && (
          <OptionRow
            title="Leave this space"
            body="You stop seeing its memories. Everyone else keeps access."
            action={
              <button onClick={() => setConfirm("leave")} className="btn btn-sm">
                <SignOutIcon {...ICON} />
                Leave space…
              </button>
            }
          />
        )}
        {lastOwner && others.length > 0 && (
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            To leave, first make someone else an owner. A shared space always needs one.
          </p>
        )}

        {isOrg && isOwner && (
          <OptionRow
            title="Delete this space"
            body="Removes it and everything in it, for everyone. This can't be undone."
            action={
              <button onClick={() => setConfirm("delete")} className="btn btn-danger btn-sm">
                <TrashIcon {...ICON} />
                Delete space…
              </button>
            }
          />
        )}
      </DetailSection>

      {error && (
        <div className="px-5 sm:px-6 pb-5">
          <ErrorLine error={error} />
        </div>
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
            undone. CopyIcon the space first if you want to keep anything.
          </p>
        ) : (
          <p>
            You will lose access to the memories in this space. Other members keep access. To return, an owner will need to invite
            you again.
          </p>
        )}
      </ConfirmDialog>
      <ConfirmDialog
        open={removing !== null}
        danger
        title={`Remove ${removing ?? ""} from ${vault.name}?`}
        confirmLabel="Remove"
        busy={removeMember.isPending}
        onCancel={() => setRemoving(null)}
        onConfirm={() =>
          removing &&
          run(
            () => removeMember.mutateAsync({ id: vault.id, email: removing }),
            "Could not remove that person. Reload and try again.",
            () => setRemoving(null),
          )
        }
      >
        <p>They lose access to this space and its memories straight away. What they added stays in the space. You can invite them again later.</p>
      </ConfirmDialog>
    </article>
  );
}

function OptionRow({ title, body, action }: { title: string; body: string; action: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
      <div className="flex-1 min-w-[200px]">
        <div className="text-[14px] font-medium">{title}</div>
        <div className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {body}
        </div>
      </div>
      <div className="shrink-0">{action}</div>
    </div>
  );
}
