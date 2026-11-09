"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { ArrowLeftIcon, ArrowsMergeIcon, CaretDownIcon, CaretRightIcon, CheckIcon, PlusIcon, StackIcon } from "@phosphor-icons/react/ssr";
import PageHeader from "@/components/ui/PageHeader";
import EmptyState from "@/components/ui/EmptyState";
import StatusLabel from "@/components/ui/StatusLabel";
import type { Vault } from "@/lib/types";
import { useMe } from "@/lib/queries/auth";
import { useConnectors, useRepointConnectors } from "@/lib/queries/connectors";
import {
  useAcceptInvitation,
  useCreateVault,
  useDeclineInvitation,
  useInvitations,
  useMergeVaults,
  useRefreshVaults,
  useVaultMembers,
  useVaults,
} from "@/lib/queries/vaults";
import { spark } from "@/components/bits/Spark";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";
import { JoinSharedSpace } from "@/components/PeerSharing";
import { cssVar, prefersReducedMotion } from "@/components/bits/motion";
import SpaceDetail from "@/components/vaults/SpaceDetail";
import { VaultShape, vaultTone } from "@/components/VaultChip";
import { ICON, JoinedNotice, ago, inviterWords, peopleLine, sameEmail } from "@/components/vaults/bits";

/* New rows (since the previous load, never the first) settle in. A stable function, so React calls it once per mount. */
function arrive(el: HTMLElement | null) {
  if (!el || prefersReducedMotion()) return;
  el.animate([{ opacity: 0.001, transform: "translateY(-4px)" }, { opacity: 1, transform: "none" }], {
    duration: 200,
    easing: cssVar("--ease-out") || "ease-out",
  });
}

function InvitationsPanel() {
  const invitationsQuery = useInvitations();
  const invitations = invitationsQuery.data ?? (invitationsQuery.isError ? [] : null);
  const acceptInvitation = useAcceptInvitation();
  const declineInvitation = useDeclineInvitation();
  const refreshVaults = useRefreshVaults();
  const [busyId, setBusyId] = useState<string | null>(null);
  const [joined, setJoined] = useState<string | null>(null);
  // the last space joined, kept after its invitation leaves the list, so the person sees where to go next
  const [welcome, setWelcome] = useState<{ id: string; name: string } | null>(null);
  const [error, setError] = useState<Failure | null>(null);

  async function accept(vaultId: string, vaultName: string, button: HTMLElement, pointer: boolean) {
    setBusyId(vaultId);
    setError(null);
    try {
      await acceptInvitation.mutateAsync(vaultId);
      if (pointer) spark(button);
      // The dashed invite turns solid with a soft flash, then the vault row arrives in the list.
      setJoined(vaultId);
      if (!prefersReducedMotion()) await new Promise((r) => setTimeout(r, 180));
      setWelcome({ id: vaultId, name: vaultName });
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
  if (!invitations || invitations.length === 0)
    return welcome ? <JoinedNotice id={welcome.id} name={welcome.name} onDismiss={() => setWelcome(null)} /> : null;

  return (
    <section className="flex flex-col gap-2.5" aria-labelledby="invites-title">
      {welcome && <JoinedNotice id={welcome.id} name={welcome.name} onDismiss={() => setWelcome(null)} />}
      <h2 id="invites-title" className="section-title flex items-center gap-2">
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
            <div className="text-[15px] font-semibold truncate">
              {inv.vault_name}
            </div>
            <div className="label mt-0.5">
              {inviterWords(inv.invited_by)} invited you as {inv.role === "owner" ? "an owner" : "a member"}
              {inv.vault_kind === "org" ? " of a team space" : ""}
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
              onClick={(e) => accept(inv.vault_id, inv.vault_name, e.currentTarget, e.detail > 0)}
              disabled={busyId === inv.vault_id}
              className="btn btn-sm"
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
  me,
  selected,
  onSelect,
}: {
  vault: Vault;
  me: string | null;
  selected: boolean;
  onSelect: () => void;
}) {
  const membersQuery = useVaultMembers(vault.id);
  const members = membersQuery.data ?? (membersQuery.isError ? [] : null);
  // Say who owns it only when it isn't you.
  const owner = vault.role !== "owner" ? members?.find((m) => m.role === "owner" && !sameEmail(m.email, me))?.email : undefined;

  return (
    <button
      onClick={onSelect}
      aria-current={selected || undefined}
      className="w-full flex items-center gap-3 px-4 py-3.5 min-h-[60px] text-left rounded-[11px] transition-[background-color] duration-[120ms] hover:bg-[var(--surface-raised)]"
      style={selected ? { background: "var(--accent-soft)", outline: "1px solid var(--accent)", outlineOffset: -1 } : undefined}
    >
      <span className="flex-1 min-w-0">
        <span className="flex items-center gap-2.5 min-w-0">
          <VaultShape tone={vaultTone(vault.kind)} />
          <span className="text-[16px] font-medium truncate">{vault.name}</span>
        </span>
        <span className="label block mt-1 truncate">
          {peopleLine(vault, members)}
          {owner ? ` · owned by ${owner}` : ""}
        </span>
      </span>
      <CaretRightIcon {...ICON} aria-hidden className="shrink-0" style={{ color: selected ? "var(--ink)" : "var(--ink-dim)" }} />
    </button>
  );
}

function CreateVaultForm({
  onCreated,
  onCancel,
}: {
  onCreated: (id: string) => void;
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
      const made = await createVault.mutateAsync({ name: trimmed });
      setName("");
      onCreated(made.id);
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

/** A quiet action that reads as a link (the page header keeps one primary button). */
const TEXT_LINK = "link inline-flex items-center gap-1 min-h-10 text-[14px] font-medium";

const vaultLabel = (v: Vault) => (v.is_default ? "Personal" : v.name);

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
        className={TEXT_LINK}
        aria-expanded={open}
        aria-controls="merge-panel"
        onClick={() => setOpen((o) => !o)}
        id="merge-title"
      >
        Advanced: combine two spaces
        <CaretDownIcon weight="duotone" size={16} aria-hidden style={{ transform: open ? "rotate(180deg)" : undefined }} />
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
            className="btn"
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
  return <LoadingSkeleton label="Loading memory spaces" rows={2} />;
}

// Wide screens show the list and the open space side by side; narrow ones show one at a time, with a way back.
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
  const meQuery = useMe();
  const me = meQuery.data?.email ?? null;
  const isAdmin = meQuery.data?.is_admin ?? false;
  const [creating, setCreating] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // On narrow screens: has the person opened a space (detail view) or are they on the list?
  const [opened, setOpened] = useState(false);
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
  // Open on the first shared space: that is where the people are.
  const selected = vaults?.find((v) => v.id === selectedId) ?? org[0] ?? personal.find((v) => v.is_default) ?? personal[0] ?? null;

  // Joining another Eunomia and combining spaces are for admins and people who run a shared space.
  const canManage = isAdmin || org.some((v) => v.role === "owner");

  const wide = useWide();
  const showDetail = wide || opened;
  const showList = wide || !opened;

  const open = (id: string) => {
    setSelectedId(id);
    setOpened(true);
    if (!wide) window.scrollTo({ top: 0 });
  };

  const list = (items: Vault[]) => (
    <div className="ledger hairline-rows">
      {items.map((v) => (
        <div key={v.id} ref={fresh.has(v.id) ? arrive : undefined}>
          <VaultRow vault={v} me={me} selected={wide && selected?.id === v.id} onSelect={() => open(v.id)} />
        </div>
      ))}
    </div>
  );

  return (
    <div className="max-w-6xl flex flex-col gap-7">
      <PageHeader
        icon={StackIcon}
        title="Memory spaces"
        intro="Keep each project's memories in its own space. Share a space with the people you invite."
        actions={
          <>
            <LiveStatusBadge />
            {/* While the create form is open, its own Create button is the one primary action. */}
            <button onClick={() => setCreating(true)} disabled={creating} className={`btn ${creating ? "" : "btn-primary"}`}>
              <PlusIcon {...ICON} />
              Create space
            </button>
          </>
        }
      />

      {creating && (
        <CreateVaultForm onCancel={() => setCreating(false)} onCreated={(id) => {
            setCreating(false);
            open(id);
          }}
        />
      )}

      {vaultsQuery.isError && <ErrorLine error={failure(vaultsQuery.error, "Could not load your memory spaces.", " Reload the page to try again.")} />}

      <InvitationsPanel />

      <div className="grid gap-6 lg:grid-cols-[320px_minmax(0,1fr)] items-start">
        {showList && (
          <div className="flex flex-col gap-7 min-w-0">
            <section className="flex flex-col gap-2.5" aria-labelledby="org-title">
              <h2 id="org-title" className="section-title flex items-center gap-2">
                Spaces
                {vaults && org.length > 0 && <span className="label tabular">{org.length}</span>}
              </h2>
              {!vaults ? (
                <SkeletonRows />
              ) : org.length === 0 ? (
                !creating && <EmptyState>No shared spaces yet.</EmptyState>
              ) : (
                list(org)
              )}
            </section>

            <section className="flex flex-col gap-2.5" aria-labelledby="personal-title">
              <h2 id="personal-title" className="section-title flex items-center gap-2">
                Personal
              </h2>
              {!vaults ? <SkeletonRows /> : list(personal)}
            </section>
          </div>
        )}

        {showDetail && (
          <div className="flex flex-col gap-3 min-w-0">
            {!wide && (
              <button type="button" className="btn btn-ghost btn-sm self-start" onClick={() => setOpened(false)}>
                <ArrowLeftIcon {...ICON} />
                All spaces
              </button>
            )}
            {selected ? (
              <SpaceDetail
                key={selected.id}
                vault={selected}
                me={me}
                onVaultsChanged={() => {
                  setSelectedId(null);
                  setOpened(false);
                }}
              />
            ) : (
              <LoadingSkeleton label="Loading memory space" layout="detail" />
            )}
          </div>
        )}
      </div>

      {showList && (
        // Anyone who can create a space can accept one shared from another Eunomia; combining
        // spaces stays with admins and owners.
        <div className="flex flex-col gap-2 items-start pt-2" style={{ borderTop: "1px solid var(--border)" }}>
          <JoinSharedSpace onJoined={(id) => open(id)} />
          {canManage && vaults && vaults.length > 1 && <MergeVaultsCard vaults={vaults} />}
        </div>
      )}
    </div>
  );
}
