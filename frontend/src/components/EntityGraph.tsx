"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useEffect, useRef, useState, type ReactNode } from "react";
import ConfirmButton from "@/components/ConfirmButton";
import { ArrowLeft, ArrowRight, Pencil, Plus, Trash2, X } from "lucide-react";
import GraphCanvas, { type CanvasInsets } from "./GraphCanvas";
import AuthorTag from "./AuthorTag";
import SyncMark from "./bits/SyncMark";
import Tooltip, { TooltipGroup } from "./bits/Tooltip";
import { cssVar, prefersReducedMotion } from "./bits/motion";
import { Blobatar } from "@blobatar/react";
import { useQueryClient } from "@tanstack/react-query";
import type { BuiltInKind, EntityDetail, EntityGraph as EntityGraphData, EntityKind } from "@/lib/types";
import { useMe } from "@/lib/queries/auth";
import {
  entityDetailQuery,
  useAddMemory,
  useAddRelation,
  useCreateEntity as useCreateEntityMutation,
  useDeleteEntity,
  useDeleteMemory,
  useEntityGraph,
  useUpdateEntity,
} from "@/lib/queries/entities";
import { useVaults } from "@/lib/queries/vaults";

const KIND_COLOR: Record<BuiltInKind, string> = {
  person: "var(--kind-person)",
  organisation: "var(--kind-organisation)",
  location: "var(--kind-location)",
  repository: "var(--kind-repository)",
  file: "var(--kind-file)",
  symbol: "var(--kind-symbol)",
};

const KIND_LABEL: Record<BuiltInKind, string> = {
  person: "Person",
  organisation: "Organisation",
  location: "Location",
  repository: "Repository",
  file: "File",
  symbol: "Symbol",
};

const ALL_KINDS: EntityKind[] = ["person", "organisation", "location", "repository", "file", "symbol"];

// An agent's own kind ("db", "decision") has no token of its own: a neutral dot and its name as written.
const kindColor = (kind: EntityKind) => KIND_COLOR[kind as BuiltInKind] ?? "var(--ink-faint)";
const kindLabel = (kind: EntityKind) => KIND_LABEL[kind as BuiltInKind] ?? kind;

// Compact age for a timestamp: "now", "4m", "3h", "2d", then a date.
function age(iso?: string): string | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d`;
  return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// The real author of a row, if attribution was recorded. `null` means it
// predates attribution tracking, so no tag is shown.
function Author({ email, me }: { email: string | null; me: string | null }) {
  if (!email) return null;
  return <AuthorTag name={email} title={email === me ? `${email} (you)` : email} />;
}

const ICON = { size: 14, strokeWidth: 1.75 } as const;

const SPARSE = 3; // fewer nodes than this shows the `guide`

// A memory row that just arrived from "Add memory" settles in from a slight blur, once.
function blurIn(el: HTMLElement | null) {
  if (!el || prefersReducedMotion()) return;
  el.animate([{ opacity: 0, filter: "blur(4px)" }, { opacity: 1, filter: "blur(0)" }], {
    duration: 200,
    easing: cssVar("--ease-out") || "ease-out",
  });
}

export default function EntityGraph({
  kinds,
  header,
  guide,
}: {
  kinds?: EntityKind[];
  header?: ReactNode;
  /** one sentence plus one action, shown while the graph is empty or sparse */
  guide?: ReactNode;
} = {}) {
  const shownKinds = kinds ?? ALL_KINDS;
  const queryClient = useQueryClient();
  const [vaultId, setVaultId] = useState<string | undefined>(undefined);
  const graphQuery = useEntityGraph({ kinds, vaultId });
  const graph: EntityGraphData | null = graphQuery.data ?? null;
  const error = graphQuery.error && !graph ? failure(graphQuery.error, "Could not load the entity graph.") : null;
  const myEmail = useMe().data?.email ?? null;
  const myVaults = useVaults().data ?? [];
  const createEntityMutation = useCreateEntityMutation();
  const updateEntityMutation = useUpdateEntity();
  const deleteEntityMutation = useDeleteEntity();
  const addMemoryMutation = useAddMemory();
  const deleteMemoryMutation = useDeleteMemory();
  const addRelationMutation = useAddRelation();
  const [selected, setSelected] = useState<EntityDetail | null>(null);
  // hidden rather than visible, so a custom kind that shows up later is on by default
  const [hiddenKinds, setHiddenKinds] = useState<Set<EntityKind>>(new Set());
  const [showCreate, setShowCreate] = useState(false);
  const [createForm, setCreateForm] = useState({ kind: shownKinds[0], name: "", aliases: "" });
  const [createError, setCreateError] = useState<Failure | null>(null);
  const [creating, setCreating] = useState(false);

  const [editing, setEditing] = useState(false);
  const [editForm, setEditForm] = useState({ name: "", aliases: "", summary: "" });
  const [memoryText, setMemoryText] = useState("");
  const [relForm, setRelForm] = useState({ to: "", label: "" });
  const [actionError, setActionError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  // "Add memory" progress: spinner while saving, a drawn check for 1.2s after
  const [memoryStatus, setMemoryStatus] = useState<"idle" | "running" | "done">("idle");
  const [freshMemory, setFreshMemory] = useState<string | null>(null);
  const memoryTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(memoryTimer.current), []);

  // px of canvas covered by the floating header/toolbar (top) and, on desktop,
  // the inspector column (right), so the camera fits nodes into what is left.
  const chromeRef = useRef<HTMLDivElement>(null);
  const [insets, setInsets] = useState<CanvasInsets>({ top: 0, right: 0, bottom: 0, left: 0 });
  useEffect(() => {
    const el = chromeRef.current;
    if (!el) return;
    const measure = () => {
      const md = window.matchMedia("(min-width: 768px)").matches;
      setInsets({ top: el.offsetTop + el.offsetHeight, right: md ? 400 : 0, bottom: md ? 48 : 16, left: 0 });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    window.addEventListener("resize", measure);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [graph]);

  function toggleKind(kind: EntityKind) {
    setHiddenKinds((prev) => {
      const next = new Set(prev);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }

  useEffect(() => {
    if (!selected && !showCreate) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (showCreate) setShowCreate(false);
      else if (!editing) setSelected(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected, showCreate, editing]);

  async function selectNode(id: string) {
    setEditing(false);
    setMemoryText("");
    setRelForm({ to: "", label: "" });
    setActionError(null);
    try {
      const detail = await queryClient.fetchQuery(entityDetailQuery(id));
      setSelected(detail);
      return detail;
    } catch {
      setSelected(null);
    }
  }

  async function createEntity(e: React.FormEvent) {
    e.preventDefault();
    const name = createForm.name.trim();
    if (!name) return;
    setCreating(true);
    setCreateError(null);
    try {
      const aliases = createForm.aliases
        .split(",")
        .map((a) => a.trim())
        .filter(Boolean);
      const created = await createEntityMutation.mutateAsync({
        kind: createForm.kind,
        name,
        aliases: aliases.length ? aliases : undefined,
        vault_id: vaultId,
      });
      setShowCreate(false);
      setCreateForm({ kind: shownKinds[0], name: "", aliases: "" });
      await selectNode(created.id);
    } catch (err) {
      setCreateError(failure(err, "Could not create the entity."));
    } finally {
      setCreating(false);
    }
  }

  function startEditing() {
    if (!selected) return;
    setEditForm({ name: selected.name, aliases: selected.aliases.join(", "), summary: selected.summary });
    setActionError(null);
    setEditing(true);
  }

  async function saveEdit(e: React.FormEvent) {
    e.preventDefault();
    if (!selected) return;
    setBusy(true);
    setActionError(null);
    try {
      const aliases = editForm.aliases
        .split(",")
        .map((a) => a.trim())
        .filter(Boolean);
      await updateEntityMutation.mutateAsync({ id: selected.id, name: editForm.name.trim(), aliases, summary: editForm.summary.trim() });
      setEditing(false);
      await selectNode(selected.id);
    } catch (err) {
      setActionError(failure(err, "Could not save changes."));
    } finally {
      setBusy(false);
    }
  }

  async function deleteSelected() {
    if (!selected) return;
    setBusy(true);
    setActionError(null);
    try {
      await deleteEntityMutation.mutateAsync(selected.id);
      setSelected(null);
    } catch (err) {
      setActionError(failure(err, "Could not delete this entity."));
      setBusy(false);
    }
  }

  async function submitMemory(e: React.FormEvent) {
    e.preventDefault();
    if (!selected || !memoryText.trim()) return;
    setBusy(true);
    setActionError(null);
    clearTimeout(memoryTimer.current);
    setMemoryStatus("running");
    const before = new Set(selected.memory.map((m) => m.id));
    try {
      await addMemoryMutation.mutateAsync({ id: selected.id, text: memoryText.trim() });
      setMemoryText("");
      const detail = await selectNode(selected.id);
      setFreshMemory(detail?.memory.find((m) => !before.has(m.id))?.id ?? null);
      setMemoryStatus("done");
      memoryTimer.current = setTimeout(() => setMemoryStatus("idle"), 1200);
    } catch (err) {
      setMemoryStatus("idle");
      setActionError(failure(err, "Could not add that memory."));
    } finally {
      setBusy(false);
    }
  }

  async function deleteMemory(memoryId: string) {
    if (!selected) return;
    setBusy(true);
    setActionError(null);
    try {
      await deleteMemoryMutation.mutateAsync(memoryId);
      await selectNode(selected.id);
    } catch (err) {
      setActionError(failure(err, "Could not delete that memory."));
    } finally {
      setBusy(false);
    }
  }

  async function submitRelation(e: React.FormEvent) {
    e.preventDefault();
    if (!selected || !relForm.to || !relForm.label.trim()) return;
    setBusy(true);
    setActionError(null);
    try {
      await addRelationMutation.mutateAsync({ id: selected.id, to_id: relForm.to, label: relForm.label.trim() });
      setRelForm({ to: "", label: "" });
      await selectNode(selected.id);
    } catch (err) {
      setActionError(failure(err, "Could not add that relation."));
    } finally {
      setBusy(false);
    }
  }

  // Everything floats over one full-bleed canvas: header and toolbar top left
  // at the same page inset as every other route (main's px-4 py-6, md px-10
  // py-8), inspector top right, zoom bottom right (inside GraphCanvas).
  const frame = (toolbar: ReactNode, content: ReactNode, inspector?: ReactNode, aside?: ReactNode) => (
    <div className="absolute inset-0 overflow-hidden">
      <div className="absolute inset-0">{content}</div>
      <div
        ref={chromeRef}
        // the inspector column is always kept clear, so opening it never reflows the toolbar or moves the camera
        className="absolute left-0 top-0 right-0 md:right-auto px-4 pt-6 md:px-10 md:pt-8 flex flex-col gap-3 items-start pointer-events-none max-w-full md:max-w-[calc(100%-25rem)] [&>*]:pointer-events-auto"
      >
        {header}
        {toolbar}
        {aside}
      </div>
      {inspector}
    </div>
  );

  if (error) {
    return frame(
      null,
      <div className="h-full flex items-center justify-center p-4">
        <div className="max-w-sm flex flex-col gap-2">
          <ErrorLine error={error} />
          <button type="button" className="btn btn-sm self-start" onClick={() => window.location.reload()}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  if (!graph) {
    return frame(
      <div className="panel p-1.5 flex gap-1.5" aria-busy="true" aria-label="Loading entity graph">
        {[64, 92, 80, 112].map((w) => (
          <span key={w} className="skeleton h-6" style={{ width: w }} />
        ))}
      </div>,
      <div className="h-full flex items-center justify-center" aria-hidden>
        <div className="relative w-48 h-48">
          {[
            [20, 30],
            [70, 12],
            [55, 60],
            [15, 75],
            [82, 70],
          ].map(([x, y]) => (
            <span key={`${x}${y}`} className="skeleton absolute w-3 h-3 rounded-full" style={{ left: `${x}%`, top: `${y}%`, borderRadius: 999 }} />
          ))}
        </div>
      </div>
    );
  }

  // the full graph (no `kinds` given) also carries agents' custom kinds: one filter each
  const filterKinds = kinds
    ? shownKinds
    : [...shownKinds, ...[...new Set(graph.nodes.map((n) => n.kind))].filter((k) => !ALL_KINDS.includes(k)).sort()];
  const visibleKinds = new Set(filterKinds.filter((k) => !hiddenKinds.has(k)));
  const counts = new Map<EntityKind, number>();
  graph.nodes.forEach((n) => counts.set(n.kind, (counts.get(n.kind) ?? 0) + 1));
  const nameOf = (id: string) => graph.nodes.find((n) => n.id === id)?.name ?? id;

  const createModal = showCreate && (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4 fade-in"
      style={{ background: "var(--scrim)" }}
      onClick={() => setShowCreate(false)}
    >
      <form
        onSubmit={createEntity}
        onClick={(e) => e.stopPropagation()}
        className="panel pop-in p-5 w-full max-w-sm flex flex-col gap-3.5"
        style={{ boxShadow: "var(--shadow-pop)" }}
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-entity-title"
      >
        <div className="flex items-center justify-between">
          <h2 id="new-entity-title" className="section-title">
            New entity
          </h2>
          <button type="button" className="btn btn-ghost btn-sm btn-icon w-[26px]" onClick={() => setShowCreate(false)} aria-label="Close">
            <X {...ICON} />
          </button>
        </div>
        <div className="label flex flex-col gap-1.5">
          Kind
          <Select
            aria-label="Kind"
            className="h-8 text-[13px] w-full"
            value={createForm.kind}
            onChange={(v) => setCreateForm((f) => ({ ...f, kind: v as EntityKind }))}
            options={filterKinds.map((k) => ({ value: k, label: kindLabel(k) }))}
          />
        </div>
        <label className="label flex flex-col gap-1.5">
          Name
          <input
            autoFocus
            className="field h-8 px-2.5 text-[13px]"
            value={createForm.name}
            onChange={(e) => setCreateForm((f) => ({ ...f, name: e.target.value }))}
            placeholder="e.g. Jordan Blake"
          />
        </label>
        <label className="label flex flex-col gap-1.5">
          Aliases (optional, comma-separated)
          <input
            className="field h-8 px-2.5 text-[13px]"
            value={createForm.aliases}
            onChange={(e) => setCreateForm((f) => ({ ...f, aliases: e.target.value }))}
            placeholder="e.g. JB, Jordy"
          />
        </label>
        {createError && <ErrorLine error={createError} />}
        <div className="flex justify-end gap-2 pt-1">
          <button type="button" className="btn btn-ghost" onClick={() => setShowCreate(false)}>
            Cancel
          </button>
          <button type="submit" disabled={creating || !createForm.name.trim()} className="btn btn-primary">
            {creating ? "Creating…" : "Create"}
          </button>
        </div>
      </form>
    </div>
  );

  const vaultSwitcher = myVaults.length > 1 && (
    <Select
      className="h-6 text-[12px] w-36"
      value={vaultId ?? ""}
      onChange={(v) => {
        setVaultId(v || undefined);
        setSelected(null);
      }}
      aria-label="Vault"
      options={myVaults.map((v) => ({ value: v.kind === "personal" ? "" : v.id, label: v.kind === "personal" ? "Personal" : v.name }))}
    />
  );

  const toolbar = (
    <div className="panel p-1.5 flex flex-wrap items-center gap-1.5 max-w-full">
      {vaultSwitcher}
      {vaultSwitcher && <span className="w-px h-4 shrink-0" style={{ background: "var(--border)" }} aria-hidden />}
      <div className="flex flex-wrap gap-1 items-center" role="group" aria-label="Entity kinds">
        {filterKinds.map((kind) => {
          const active = visibleKinds.has(kind);
          return (
            <button key={kind} type="button" className="pill shrink-0" aria-pressed={active} onClick={() => toggleKind(kind)}>
              <span
                className="w-2 h-2 rounded-full shrink-0"
                style={{ background: kindColor(kind), opacity: active ? 1 : 0.35 }}
                aria-hidden
              />
              {kindLabel(kind)}
              <span className="font-mono" style={{ color: "var(--ink-faint)" }}>
                {counts.get(kind) ?? 0}
              </span>
            </button>
          );
        })}
      </div>
      <span className="w-px h-4 shrink-0" style={{ background: "var(--border)" }} aria-hidden />
      {graph.nodes.length > 0 && (
        <form
          className="shrink-0"
          onSubmit={(e) => {
            e.preventDefault();
            const input = e.currentTarget.elements.namedItem("find") as HTMLInputElement;
            const q = input.value.trim().toLowerCase();
            const hit = graph.nodes.find((n) => n.name.toLowerCase() === q) ?? graph.nodes.find((n) => n.name.toLowerCase().includes(q));
            if (q && hit) {
              setHiddenKinds((prev) => {
                const next = new Set(prev);
                next.delete(hit.kind);
                return next;
              });
              selectNode(hit.id);
              input.value = "";
            }
          }}
        >
          <input
            name="find"
            list="entity-names"
            className="field h-6 w-36 px-2 text-[12px]"
            placeholder="Find entity…"
            aria-label="Find entity"
            autoComplete="off"
          />
          <datalist id="entity-names">
            {graph.nodes.map((n) => (
              <option key={n.id} value={n.name} />
            ))}
          </datalist>
        </form>
      )}
      <button type="button" className="btn btn-sm shrink-0" onClick={() => setShowCreate(true)}>
        <Plus size={13} strokeWidth={1.75} />
        New entity
      </button>
    </div>
  );

  if (graph.nodes.length === 0) {
    return (
      <>
        {frame(
          toolbar,
          <div className="h-full flex items-center justify-center p-6">
            {guide ? (
              <div className="flex flex-col items-center gap-3 text-center max-w-xs text-[13px]" style={{ color: "var(--ink-dim)" }}>
                <p>No entities yet.</p>
                {guide}
              </div>
            ) : (
              <div className="flex flex-col items-center gap-3 text-center max-w-xs">
                <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                  No entities yet. They appear as sources sync and agents write memory, or add one yourself.
                </p>
                <button type="button" className="btn btn-primary btn-sm" onClick={() => setShowCreate(true)}>
                  <Plus size={13} strokeWidth={1.75} />
                  New entity
                </button>
              </div>
            )}
          </div>
        )}
        {createModal}
      </>
    );
  }

  const lastTouched = selected
    ? selected.memory.reduce<string | undefined>((a, m) => (m.created_at && (!a || m.created_at > a) ? m.created_at : a), undefined)
    : undefined;

  const inspector = selected && (
    <aside
      aria-label={`${selected.name} details`}
      className="panel frame-selected pop-in absolute z-20 flex flex-col inset-x-2 bottom-2 max-h-[62%] md:inset-x-auto md:bottom-auto md:right-6 md:top-6 md:w-[22rem] md:max-h-[calc(100%-5rem)]"
      style={{ transformOrigin: "top right", boxShadow: "var(--shadow-pop)" }}
    >
      <div className="flex items-start justify-between gap-2 p-4 pb-3">
        <div className="flex items-center gap-3 min-w-0">
          {selected.kind === "person" && <Blobatar name={selected.name || selected.id} animate="hover" size={36} background="circle" />}
          <div className="min-w-0">
            <h2 className="text-[16px] font-semibold tracking-[-0.015em] truncate">{selected.name}</h2>
            <span className="text-[12px] inline-flex items-center gap-1.5 mt-0.5" style={{ color: "var(--ink-dim)" }}>
              <span className="w-2 h-2 rounded-full" style={{ background: kindColor(selected.kind) }} aria-hidden />
              {kindLabel(selected.kind)}
            </span>
          </div>
        </div>
        <TooltipGroup>
          <div className="flex items-center gap-0.5 shrink-0 -mr-1">
            <Tooltip label="Edit">
              <button className="btn btn-ghost btn-sm btn-icon w-[26px]" onClick={startEditing} aria-label="Edit">
                <Pencil {...ICON} />
              </button>
            </Tooltip>
            <Tooltip label="Delete. Also removes its memory and relations">
              <ConfirmButton label="Delete" confirmLabel="Delete" onConfirm={deleteSelected} disabled={busy} className="btn btn-ghost btn-sm btn-icon w-[26px]">
                <Trash2 {...ICON} />
              </ConfirmButton>
            </Tooltip>
            <Tooltip label="Close" shortcut="Esc">
              <button className="btn btn-ghost btn-sm btn-icon w-[26px]" onClick={() => setSelected(null)} aria-label="Close">
                <X {...ICON} />
              </button>
            </Tooltip>
          </div>
        </TooltipGroup>
      </div>

      <div className="overflow-y-auto px-4 pb-4 flex flex-col gap-6">
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-[13px] items-center">
          <dt className="label">Added by</dt>
          <dd className="min-w-0">
            {selected.owner_email ? <Author email={selected.owner_email} me={myEmail} /> : <span style={{ color: "var(--ink-faint)" }}>Not recorded</span>}
          </dd>
          <dt className="label">Last memory</dt>
          <dd className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {lastTouched ? `${age(lastTouched)} ago`.replace("now ago", "just now") : "None yet"}
          </dd>
          {selected.aliases.length > 0 && (
            <>
              <dt className="label">Aliases</dt>
              <dd className="truncate" style={{ color: "var(--ink-dim)" }}>
                {selected.aliases.join(", ")}
              </dd>
            </>
          )}
        </dl>

        {actionError && <ErrorLine error={actionError} />}

        {editing ? (
          <form onSubmit={saveEdit} className="flex flex-col gap-2.5">
            <label className="label flex flex-col gap-1">
              Name
              <input className="field h-8 px-2.5 text-[13px]" value={editForm.name} onChange={(e) => setEditForm((f) => ({ ...f, name: e.target.value }))} />
            </label>
            <label className="label flex flex-col gap-1">
              Aliases (comma-separated)
              <input className="field h-8 px-2.5 text-[13px]" value={editForm.aliases} onChange={(e) => setEditForm((f) => ({ ...f, aliases: e.target.value }))} />
            </label>
            <label className="label flex flex-col gap-1">
              Summary
              <textarea
                className="field px-2.5 py-1.5 text-[13px]"
                rows={3}
                value={editForm.summary}
                onChange={(e) => setEditForm((f) => ({ ...f, summary: e.target.value }))}
              />
            </label>
            <div className="flex gap-2">
              <button type="submit" disabled={busy} className="btn btn-primary btn-sm">
                Save
              </button>
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => setEditing(false)}>
                Cancel
              </button>
            </div>
          </form>
        ) : (
          selected.summary && (
            <p className="text-[13px] leading-relaxed -mt-2" style={{ color: "var(--ink-dim)" }}>
              {selected.summary}
            </p>
          )
        )}

        <section className="flex flex-col gap-2">
          <h3 className="section-title flex items-baseline gap-2">
            Memory
            <span className="font-mono text-[12px] font-normal" style={{ color: "var(--ink-faint)" }}>
              {selected.memory.length}
            </span>
          </h3>
          {selected.memory.length === 0 ? (
            <p className="text-[12.5px]" style={{ color: "var(--ink-faint)" }}>
              Nothing recorded yet. Add the first fact below.
            </p>
          ) : (
            <ul className="hairline-rows">
              {selected.memory.map((m) => (
                <li key={m.id} ref={m.id === freshMemory ? blurIn : undefined} className="group py-2.5 flex items-start justify-between gap-2">
                  <div className="min-w-0 flex flex-col gap-1.5">
                    <span className="text-[13px] leading-snug">{m.text}</span>
                    {(m.owner_email || m.created_at) && (
                      <div className="flex items-center gap-2 min-w-0">
                        <Author email={m.owner_email} me={myEmail} />
                        {age(m.created_at) && (
                          <time className="font-mono text-[11.5px]" style={{ color: "var(--ink-faint)" }} dateTime={m.created_at} title={m.created_at}>
                            {age(m.created_at)}
                          </time>
                        )}
                      </div>
                    )}
                  </div>
                  <Tooltip label="Delete memory">
                    <ConfirmButton
                      label="Delete memory"
                      confirmLabel="Delete"
                      onConfirm={() => deleteMemory(m.id)}
                      disabled={busy}
                      className="btn btn-ghost btn-sm btn-icon w-[24px] h-[24px] shrink-0 -mr-1"
                    >
                      <X size={13} strokeWidth={1.75} />
                    </ConfirmButton>
                  </Tooltip>
                </li>
              ))}
            </ul>
          )}
          <form onSubmit={submitMemory} className="flex flex-col gap-2">
            <textarea
              className="field px-2.5 py-1.5 text-[13px]"
              rows={2}
              placeholder="Add a memory about this entity…"
              value={memoryText}
              onChange={(e) => setMemoryText(e.target.value)}
            />
            <div className="flex items-center gap-2">
              <button type="submit" disabled={busy || !memoryText.trim()} className="btn btn-sm">
                Add memory
              </button>
              {memoryStatus !== "idle" && (
                <span style={{ color: "var(--ink-faint)" }}>
                  <SyncMark status={memoryStatus} />
                </span>
              )}
            </div>
          </form>
        </section>

        <section className="flex flex-col gap-2">
          <h3 className="section-title flex items-baseline gap-2">
            Relations
            <span className="font-mono text-[12px] font-normal" style={{ color: "var(--ink-faint)" }}>
              {selected.relations.length}
            </span>
          </h3>
          {selected.relations.length === 0 ? (
            <p className="text-[12.5px]" style={{ color: "var(--ink-faint)" }}>
              No known relations.
            </p>
          ) : (
            <ul className="hairline-rows">
              {selected.relations.map((r) => {
                const other = r.direction === "out" ? r.out : r.in;
                return (
                  <li key={r.id} className="py-2 flex items-center gap-2 min-w-0">
                    {r.direction === "out" ? (
                      <ArrowRight size={13} strokeWidth={1.75} className="shrink-0" style={{ color: "var(--ink-faint)" }} aria-label="outgoing" />
                    ) : (
                      <ArrowLeft size={13} strokeWidth={1.75} className="shrink-0" style={{ color: "var(--ink-faint)" }} aria-label="incoming" />
                    )}
                    <span className="font-mono text-[11.5px] shrink-0" style={{ color: "var(--ink-faint)" }}>
                      {r.label}
                    </span>
                    <button
                      type="button"
                      className="text-[13px] truncate text-left min-w-0 hover:underline"
                      style={{ color: "var(--accent-text)" }}
                      onClick={() => selectNode(other)}
                    >
                      {nameOf(other)}
                    </button>
                    <span className="ml-auto shrink-0">
                      <Author email={r.owner_email} me={myEmail} />
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
          <form onSubmit={submitRelation} className="flex flex-col gap-2">
            <Select
              aria-label="Relate to"
              placeholder="Relate to…"
              className="h-8 text-[13px] w-full"
              value={relForm.to}
              onChange={(v) => setRelForm((f) => ({ ...f, to: v }))}
              options={graph.nodes
                .filter((n) => n.id !== selected.id)
                .map((n) => ({ value: n.id, label: n.name, hint: kindLabel(n.kind) }))}
            />
            <input
              className="field h-8 px-2.5 text-[13px]"
              placeholder="Relation label, e.g. works_at"
              aria-label="Relation label"
              value={relForm.label}
              onChange={(e) => setRelForm((f) => ({ ...f, label: e.target.value }))}
            />
            <button type="submit" disabled={busy || !relForm.to || !relForm.label.trim()} className="btn btn-sm self-start">
              Add relation
            </button>
          </form>
        </section>
      </div>
    </aside>
  );

  return (
    <>
      {frame(
        toolbar,
        <>
          <GraphCanvas
            nodes={graph.nodes}
            edges={graph.edges}
            kindColor={kindColor}
            visibleKinds={visibleKinds}
            insets={insets}
            selectedId={selected?.id}
            onSelect={selectNode}
            myEmail={myEmail}
            ariaLabel="Entity relationship graph"
          />
        </>,
        inspector,
        guide && graph.nodes.length < SPARSE && (
          <div className="panel px-3 py-2.5 max-w-sm text-[13px] flex flex-col gap-2 items-start" style={{ color: "var(--ink-dim)" }}>
            {guide}
          </div>
        )
      )}
      {createModal}
    </>
  );
}
