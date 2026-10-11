"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  Check,
  ChevronRight,
  Combine,
  Copy,
  Lock,
  LogOut,
  Mail,
  Pencil,
  Plus,
  Trash2,
  UserPlus,
  Users,
  X,
} from "lucide-react";
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
  useVaultMembers,
  useVaults,
} from "@/lib/queries/vaults";
import HoldButton from "@/components/bits/HoldButton";
import SyncMark from "@/components/bits/SyncMark";
import { spark } from "@/components/bits/Spark";
import Tooltip from "@/components/bits/Tooltip";
import { cssVar, prefersReducedMotion } from "@/components/bits/motion";

const ICON = { size: 14, strokeWidth: 1.75 } as const;
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
      className="inline-flex items-center h-5 px-1.5 rounded-[4px] text-[11.5px] font-medium shrink-0"
      style={{
        border: "var(--hair) solid var(--border-strong)",
        color: role === "owner" ? "var(--ink)" : "var(--ink-dim)",
      }}
    >
      {role === "owner" ? "Owner" : "Member"}
    </span>
  );
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
  const already = members?.some(
    (m) => m.email.toLowerCase() === trimmed.toLowerCase(),
  );
  const created = ago(vault.created_at);

  async function invite() {
    setTouched(true);
    if (!trimmed || !EMAIL_RE.test(trimmed)) return;
    setBusy(true);
    setError(null);
    setSent(null);
    try {
      await inviteMember.mutateAsync({ id: vault.id, email: trimmed, role });
      setSent(
        `Invite sent. It shows up on their Vaults page until they join or decline.`,
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
    await run(() => renameVault.mutateAsync({ id: vault.id, name }), "Could not rename this vault. Try a different name.", () => setRenaming(false));
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
      setError(failure(e, "Could not clone this vault. Try a different name."));
    } finally {
      setCloneBusy(false);
    }
  }

  return (
    <aside
      className={inline ? "flex flex-col" : "panel flex flex-col"}
      style={
        inline ? { borderTop: "var(--hair) solid var(--border)" } : undefined
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
              aria-label="Vault name"
              autoFocus
              maxLength={80}
              className="field h-8 px-3 text-[14px] flex-1 min-w-0"
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
          <div className="flex items-center gap-2 min-w-0">
            <h2 className="text-[17px] font-semibold tracking-[-0.015em] truncate">
              {vault.name}
            </h2>
            <RoleChip role={vault.role} />
            {isOwner && (
              <Tooltip label="Rename vault">
                <button
                  onClick={() => {
                    setRenameValue(vault.name);
                    setError(null);
                    setRenaming(true);
                  }}
                  aria-label="Rename vault"
                  className="btn btn-ghost btn-sm btn-icon shrink-0"
                >
                  <Pencil {...ICON} />
                </button>
              </Tooltip>
            )}
          </div>
        )}
        <span className="label">
          {isOrg
            ? "Org vault · shared with every member below"
            : "Personal vault · private to you, nobody else can be added"}
          {created ? ` · created ${created}` : ""}
        </span>
      </div>

      <div
        className="px-5 pb-4 flex flex-col gap-2"
        style={{ borderTop: "var(--hair) solid var(--border)" }}
      >
        <h3 className="label pt-4 flex items-center gap-1.5">
          Members
          {members && <span className="font-mono">{members.length}</span>}
        </h3>
        <ul className="hairline-rows">
          {members === null &&
            [0, 1].map((i) => (
              <li key={i} className="flex items-center gap-2 h-10">
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
                className="flex items-center gap-2 min-h-10 py-1.5"
              >
                <span className="flex max-w-[96px] min-w-0 shrink-0">
                  <AuthorTag name={m.email} />
                </span>
                <span
                  className="flex-1 min-w-0 truncate text-[12.5px]"
                  style={{ color: "var(--ink-dim)" }}
                >
                  {m.email}
                  {isMe && (
                    <span style={{ color: "var(--ink-faint)" }}> (you)</span>
                  )}
                </span>
                {removing === m.email ? (
                  <span className="inline-flex items-center gap-1 shrink-0">
                    <button
                      onClick={() =>
                        run(
                          () => removeMember.mutateAsync({ id: vault.id, email: m.email }),
                          "Could not remove that member. Reload and try again.",
                          () => setRemoving(null),
                        )
                      }
                      className="btn btn-danger btn-sm"
                    >
                      Remove
                    </button>
                    <button
                      onClick={() => setRemoving(null)}
                      className="btn btn-ghost btn-sm btn-icon"
                      aria-label="Keep member"
                    >
                      <X {...ICON} />
                    </button>
                  </span>
                ) : (
                  <>
                    <RoleChip role={m.role} />
                    {isOwner && !isMe && (
                      <Tooltip label={`Remove ${m.email}`}>
                        <button
                          onClick={() => setRemoving(m.email)}
                          aria-label="Remove member"
                          className="btn btn-ghost btn-sm btn-icon shrink-0"
                        >
                          <Trash2 {...ICON} />
                        </button>
                      </Tooltip>
                    )}
                  </>
                )}
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
            <label htmlFor={`invite-${vault.id}`} className="label">
              Invite by email
            </label>
            <div className="flex items-center gap-1.5">
              <input
                id={`invite-${vault.id}`}
                type="email"
                className="field h-8 px-3 text-[13px] flex-1 min-w-0"
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
                className="h-8 text-[12.5px] w-28 shrink-0"
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
                className="text-[12px] pt-1.5"
                style={{
                  color: invalid ? "var(--critical)" : "var(--ink-faint)",
                }}
              >
                {invalid
                  ? "That isn't an email address. Use name@company.com."
                  : already
                    ? "Already a member."
                    : (sent ??
                      "They join once they accept. Owners can invite and remove.")}
              </span>
              <button
                type="submit"
                disabled={busy || !trimmed}
                className="btn btn-sm shrink-0"
              >
                {busy || sent ? <SyncMark status={busy ? "running" : "done"} /> : <UserPlus {...ICON} />}
                {busy ? "Inviting…" : "Invite"}
              </button>
            </div>
          </form>
        )}
      </div>

      <div
        className="px-5 py-4 flex flex-col gap-2"
        style={{ borderTop: "var(--hair) solid var(--border)" }}
      >
        {cloning ? (
          <form
            className="flex flex-col gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              doClone();
            }}
          >
            <label htmlFor={`clone-${vault.id}`} className="label">
              Name for the copy. {vault.name} stays exactly as it is.
            </label>
            <input
              id={`clone-${vault.id}`}
              autoFocus
              className="field h-8 px-3 text-[13px]"
              value={cloneName}
              onChange={(e) => setCloneName(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && setCloning(false)}
            />
            <div className="flex justify-end gap-1.5">
              <button
                type="button"
                onClick={() => setCloning(false)}
                className="btn btn-ghost btn-sm"
              >
                Cancel
              </button>
              <button type="submit" disabled={cloneBusy} className="btn btn-sm">
                {cloneBusy ? "Cloning…" : "Clone"}
              </button>
            </div>
          </form>
        ) : confirm ? (
          <div
            className="flex flex-col gap-2 rounded-[7px] p-3"
            style={{ background: "var(--critical-soft)" }}
          >
            <p className="text-[12.5px]" style={{ color: "var(--ink)" }}>
              {confirm === "delete"
                ? `Delete ${vault.name}? Its entities and memory are removed for every member. This can't be undone.`
                : `Leave ${vault.name}? You lose access until an owner invites you again.`}
            </p>
            <div className="flex justify-end gap-1.5">
              <button
                onClick={() => setConfirm(null)}
                className="btn btn-ghost btn-sm"
              >
                Cancel
              </button>
              <button
                onClick={() =>
                  confirm === "delete"
                    ? run(
                        () => deleteVault.mutateAsync(vault.id),
                        "Could not delete this vault. Reload and try again.",
                        onVaultsChanged,
                      )
                    : run(
                        () => leaveVault.mutateAsync(vault.id),
                        "Could not leave this vault. It needs an owner, so make someone else owner first.",
                        onVaultsChanged,
                      )
                }
                disabled={leaveVault.isPending || deleteVault.isPending}
                className="btn btn-danger btn-sm"
              >
                {confirm === "delete" ? (
                  <Trash2 {...ICON} />
                ) : (
                  <LogOut {...ICON} />
                )}
                {confirm === "delete" ? "Delete vault" : "Leave"}
              </button>
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-1.5">
            <button
              onClick={() => {
                setCloneName(`${vault.name} (copy)`);
                setCloning(true);
              }}
              className="btn btn-sm"
            >
              {cloned ? <DrawnCheck /> : <Copy {...ICON} />}
              Clone vault
            </button>
            <span className="flex-1" />
            {isOrg && (
              <button
                onClick={() => setConfirm("leave")}
                className="btn btn-ghost btn-sm"
              >
                <LogOut {...ICON} />
                Leave this vault
              </button>
            )}
            {isOrg && isOwner && (
              // Click still opens the confirm step; holding 900ms is the accelerator.
              <Tooltip label="Hold to delete now">
              <HoldButton
                holdMs={900}
                onClick={() => setConfirm("delete")}
                onConfirm={() =>
                  run(
                    () => deleteVault.mutateAsync(vault.id),
                    "Could not delete this vault. Reload and try again.",
                    onVaultsChanged,
                  )
                }
                className="btn-sm"
                aria-label="Delete vault"
              >
                <Trash2 {...ICON} />
                Delete
              </HoldButton>
              </Tooltip>
            )}
          </div>
        )}
        {membersQuery.isError && <ErrorLine error={failure(membersQuery.error, "Could not load the members of this vault.")} />}
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
        <Mail {...ICON} aria-hidden style={{ color: "var(--ink-faint)" }} />
        Pending invitations
        <span className="label font-mono">{invitations.length}</span>
      </h2>
      {invitations.map((inv) => (
        <div
          key={inv.vault_id}
          className="ledger relative flex flex-wrap items-center gap-3 px-4 py-3"
          style={{ borderStyle: joined === inv.vault_id ? "solid" : "dashed", borderColor: "var(--border-strong)" }}
        >
          {joined === inv.vault_id && <JoinFlash />}
          <div className="flex-1 min-w-[160px]">
            <div className="text-[13.5px] font-medium truncate">
              {inv.vault_name}
            </div>
            <div className="label mt-0.5">
              Invited as {inv.role === "owner" ? "an owner" : "a member"}
              {inv.vault_kind === "org" ? " of an org vault" : ""}
              {ago(inv.created_at) ? ` · ${ago(inv.created_at)}` : ""}
            </div>
          </div>
          <div className="flex items-center gap-1.5">
            <button
              onClick={() => decline(inv.vault_id)}
              disabled={busyId === inv.vault_id}
              aria-label="Decline invitation"
              className="btn btn-ghost btn-sm"
            >
              <X {...ICON} />
              Decline
            </button>
            <button
              onClick={(e) => accept(inv.vault_id, e.currentTarget, e.detail > 0)}
              disabled={busyId === inv.vault_id}
              className="btn btn-primary btn-sm"
            >
              <Check {...ICON} />
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
      className={`w-full first:rounded-t-[10px] last:rounded-b-[10px] flex items-center gap-3 px-4 py-3 text-left transition-[background-color,transform] duration-[120ms] active:scale-[0.995] hover:bg-[var(--surface-raised)] ${selected ? "frame-selected" : ""}`}
      style={selected ? { background: "var(--accent-soft)" } : undefined}
    >
      <span className="flex-1 min-w-0">
        <span className="flex items-center gap-2 min-w-0">
          <span className="text-[13.5px] font-medium truncate">
            {vault.name}
          </span>
          <RoleChip role={vault.role} />
        </span>
        <span className="label block mt-0.5 truncate">
          {vault.kind === "personal" ? "Private to you" : "Org vault"}
          {members
            ? ` · ${members.length} ${members.length === 1 ? "member" : "members"}`
            : ""}
        </span>
      </span>
      <MemberStack members={members} />
      <ChevronRight
        {...ICON}
        aria-hidden
        className="shrink-0"
        style={{ color: selected ? "var(--accent-text)" : "var(--ink-faint)" }}
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
        failure(e, "Could not create that vault. Try a different name."),
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="ledger pop-in flex flex-col gap-1.5 px-4 py-3"
      style={{ transformOrigin: "top right" }}
      onSubmit={(e) => {
        e.preventDefault();
        create();
      }}
    >
      <label htmlFor="new-vault-name" className="label">
        Name the vault. You become its owner and can invite people next.
      </label>
      <div className="flex flex-wrap items-center gap-2">
        <input
          id="new-vault-name"
          autoFocus
          className="field h-8 px-3 text-[13px] flex-1 min-w-[180px]"
          placeholder="Vault name, e.g. a project, service, client, team or homelab"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && onCancel()}
        />
        <button
          type="button"
          onClick={onCancel}
          className="btn btn-ghost btn-sm"
        >
          Cancel
        </button>
        <button
          type="submit"
          disabled={busy || !name.trim()}
          className="btn btn-sm"
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
      setRepointed({ ok: true, text: `${r.connectors === 1 ? "1 connector now files" : `${r.connectors} connectors now file`} into “${mergedInto.name}”. Records already synced stay put until you move them from the connector's page.` });
    } catch (e) {
      setRepointed({ ok: false, text: failure(e, "Could not point the connectors at the new vault.", " Try again.").message });
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
          "Could not merge those vaults. Check you are a member of both and try again.",
        ),
      );
    } finally {
      setBusy(false);
    }
  }

  const select = (value: string, set: (v: string) => void, label: string) => (
    <div className="flex flex-col gap-1.5 flex-1 min-w-[140px]">
      <span className="label">{label}</span>
      <Select
        className="h-8 text-[13px] w-full"
        value={value}
        onChange={set}
        aria-label={label}
        options={vaults.map((v) => ({ value: v.id, label: vaultLabel(v) }))}
      />
    </div>
  );

  return (
    <section className="flex flex-col gap-2.5" aria-labelledby="merge-title">
      <div>
        <h2 id="merge-title" className="section-title">
          Merge two vaults
        </h2>
        <p
          className="text-[13px] mt-1 max-w-[62ch]"
          style={{ color: "var(--ink-dim)" }}
        >
          Copies both into a brand new vault. The originals are never changed.
          Matching entities (same kind and name) are combined, and duplicate
          facts are kept once.
        </p>
      </div>
      <div className="ledger p-4 flex flex-col gap-3">
        <div className="flex items-end gap-2 flex-wrap">
          {select(a, setA, "First vault")}
          <Plus
            {...ICON}
            aria-hidden
            className="mb-2 hidden sm:block"
            style={{ color: "var(--ink-faint)" }}
          />
          {select(b, setB, "Second vault")}
        </div>
        <div className="flex items-end gap-2 flex-wrap">
          <label className="flex flex-col gap-1.5 flex-1 min-w-[180px]">
            <span className="label">New vault name</span>
            <input
              className="field h-8 px-3 text-[13px]"
              placeholder="New vault name (optional)"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
          <button
            onClick={merge}
            disabled={busy || !a || a === b}
            className="btn"
          >
            <Combine {...ICON} />
            {busy ? "Merging…" : "Merge"}
          </button>
        </div>
        {a === b && (
          <p className="label">Pick two different vaults to merge.</p>
        )}
        {result && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <p
              role="status"
              className="text-[12.5px] flex items-center gap-2"
              style={{ color: "var(--ink)" }}
            >
              <span
                className="dot"
                style={{ background: "var(--good)" }}
                aria-hidden
              />
              {result}
            </p>
            {chips && <MergeChips key={chips.id} from={chips.from} to={chips.to} />}
          </div>
        )}
        {mergedInto && affected > 0 && !repointed?.ok && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <span className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
              {affected === 1 ? "A connector still files" : `${affected} connectors still file`} into the vaults you merged.
            </span>
            <button type="button" className="btn btn-sm" onClick={pointConnectors} disabled={repoint.isPending}>
              {repoint.isPending ? "Pointing…" : `Point ${affected === 1 ? "it" : "them"} at “${mergedInto.name}”`}
            </button>
          </div>
        )}
        {repointed && (
          <p role="status" className="text-[12.5px] fade-in" style={{ color: repointed.ok ? "var(--good)" : "var(--critical)" }}>
            {repointed.text}
          </p>
        )}
        {error && <ErrorLine error={error} />}
      </div>
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
        <div key={i} className="flex items-center gap-3 px-4 py-3">
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
    <div className="max-w-6xl flex flex-col gap-9">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <h1 className="page-title">Vaults</h1>
          <p
            className="text-[13px] mt-1.5 max-w-[62ch]"
            style={{ color: "var(--ink-dim)" }}
          >
            A vault is a separate scope of entities and memory. Your personal
            vault is always private. Make more for anything you want kept apart
            (a project or service, a client, a team, your homelab), and share
            one by inviting people. Agents can refer to a vault by its name.
          </p>
        </div>
        <button
          onClick={() => setCreating(true)}
          disabled={creating}
          className="btn btn-primary"
        >
          <Plus {...ICON} />
          New vault
        </button>
      </header>

      {vaultsQuery.isError && <ErrorLine error={failure(vaultsQuery.error, "Could not load your vaults.", " Reload the page to try again.")} />}

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
              <Users
                {...ICON}
                aria-hidden
                style={{ color: "var(--ink-faint)" }}
              />
              Other vaults
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
                <div className="ledger px-4 py-4 flex flex-wrap items-center justify-between gap-3">
                  <p
                    className="text-[13px]"
                    style={{ color: "var(--ink-dim)" }}
                  >
                    No other vaults yet. Create one for anything you want kept
                    apart, or accept an invite.
                  </p>
                  <button
                    onClick={() => setCreating(true)}
                    className="btn btn-sm"
                  >
                    <Plus {...ICON} />
                    Create a vault
                  </button>
                </div>
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
              <Lock
                {...ICON}
                aria-hidden
                style={{ color: "var(--ink-faint)" }}
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
          <div className="sticky top-6">
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
