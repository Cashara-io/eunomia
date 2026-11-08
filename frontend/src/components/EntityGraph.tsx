"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { Suspense, useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { useSearchParams } from "next/navigation";
import ConfirmButton from "@/components/ConfirmButton";
import { ArrowLeftIcon, LightningIcon, PencilSimpleIcon, PlusIcon, TrashIcon, XIcon } from "@phosphor-icons/react/ssr";
import Link from "next/link";
import GraphCanvas, { type CanvasInsets } from "./GraphCanvas";
import SyncMark from "./bits/SyncMark";
import Tooltip, { TooltipGroup } from "./bits/Tooltip";
import { cssVar, prefersReducedMotion } from "./bits/motion";
import Monogram from "./graph/Monogram";
import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import type { BuiltInKind, EntityDetail, EntityGraph as EntityGraphData, EntityKind } from "@/lib/types";
import { useMe } from "@/lib/queries/auth";
import {
  entityDetailQuery,
  entityListQuery,
  findEntityVault,
  useAddMemory,
  useAddRelation,
  useCreateEntity as useCreateEntityMutation,
  useDeleteEntity,
  useDeleteMemory,
  useEntityGraph,
  useUpdateEntity,
} from "@/lib/queries/entities";
import { useVaults } from "@/lib/queries/vaults";
import EditableMemoryText from "@/components/live/EditableMemoryText";
import TidyUp from "./graph/TidyUp";
import PropertyValues from "./graph/PropertyValues";
import EmptyState from "./ui/EmptyState";
import EntityList from "./graph/EntityList";
import { chooseSpace, useSharedSpace } from "./graph/space";
import SpaceSelect from "./graph/SpaceSelect";
import { defaultKindHex, isCustomKind, kindColor, setKindColor, useKindColor } from "@/lib/kindColors";

const KIND_LABEL: Record<BuiltInKind, string> = {
  person: "Person",
  organisation: "Organisation",
  location: "Location",
  repository: "Repository",
  file: "File",
  symbol: "Symbol",
};

const ALL_KINDS: EntityKind[] = ["person", "organisation", "location", "repository", "file", "symbol"];
// Code map's kinds: off by default on People & topics, behind one "Code" chip.
const CODE_KINDS: EntityKind[] = ["repository", "file", "symbol"];

// An agent's own kind ("db", "decision") has no token of its own: it gets a stable colour from its name
// (or the one the person picked, see lib/kindColors.ts) and its name in sentence case ("project" -> "Project").
const kindLabel = (kind: EntityKind) => {
  const builtIn = KIND_LABEL[kind as BuiltInKind];
  if (builtIn) return builtIn;
  const words = kind.replace(/[_-]+/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1).toLowerCase();
};

// One time format for the whole inspector: "just now", "4m ago", "3h ago", "2d ago", then a date.
function ago(iso?: string | null): string | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

// "Added by ana@acme.test · 3h ago", as plain meta text. A row from before attribution was
// recorded shows only its age (or nothing).
function Byline({ email, at }: { email: string | null | undefined; at?: string | null }) {
  const when = ago(at);
  if (!email && !when) return null;
  return (
    <p className="text-[13px] min-w-0 truncate" style={{ color: "var(--ink-dim)" }} title={email ?? undefined}>
      {email && <>Added by {email}</>}
      {email && when && " · "}
      {when && (
        <time dateTime={at ?? undefined} title={at ?? undefined}>
          {email ? when : when.charAt(0).toUpperCase() + when.slice(1)}
        </time>
      )}
    </p>
  );
}

const ICON = { size: 18, weight: "duotone" } as const;

const LIST_LIMIT = 500; // rows the List view reads in one go
const SPARSE = 3; // fewer nodes than this shows the `guide`

// A memory row that just arrived from "Add memory" settles in from a slight blur, once.
function blurIn(el: HTMLElement | null) {
  if (!el || prefersReducedMotion()) return;
  el.animate([{ opacity: 0, filter: "blur(4px)" }, { opacity: 1, filter: "blur(0)" }], {
    duration: 200,
    easing: cssVar("--ease-out") || "ease-out",
  });
}

const CHROME_KEY = "eunomia.graph.chromeHidden";
const chromeListeners = new Set<() => void>();
let chromeFallback = false; // when the browser blocks storage, remembered for this visit only
function subscribeChrome(l: () => void) {
  chromeListeners.add(l);
  return () => chromeListeners.delete(l);
}
function readChromeHidden() {
  try {
    return window.localStorage.getItem(CHROME_KEY) === "1";
  } catch {
    return chromeFallback;
  }
}

// px of the canvas the desktop inspector covers (22rem wide, 1.5rem from the edge, plus a gap)
const INSPECTOR_RESERVE = 400;

// Phones (below md) get the inspector as a full-screen sheet.
function subscribeWidth(l: () => void) {
  const mq = window.matchMedia("(max-width: 767px)");
  mq.addEventListener("change", l);
  return () => mq.removeEventListener("change", l);
}
const readPhone = () => window.matchMedia("(max-width: 767px)").matches;

/** `?id=<entity>`, `?vault=<space>` and `?memory=<memory>` from the URL. Its own component so that only it waits on the URL (Suspense). */
function UrlFocus({ onChange }: { onChange: (id: string | null, vault: string | null, memory: string | null) => void }) {
  const params = useSearchParams();
  const id = params.get("id");
  const vault = params.get("vault");
  const memory = params.get("memory");
  useEffect(() => onChange(id, vault, memory), [id, vault, memory]); // eslint-disable-line react-hooks/exhaustive-deps
  return null;
}

/** Scrolls a memory a search result pointed at into view, once, as it mounts. */
const scrollToMemory = (el: HTMLElement | null) => el?.scrollIntoView({ block: "center", behavior: prefersReducedMotion() ? "auto" : "smooth" });

export default function EntityGraph({
  kinds,
  header,
  guide,
  mode = "graph",
}: {
  kinds?: EntityKind[];
  /** "list": the same toolbar and inspector over a plain table instead of the canvas */
  mode?: "graph" | "list";
  header?: ReactNode;
  /** one sentence plus at most one action, shown while the graph is empty */
  guide?: { text: ReactNode; action?: ReactNode };
} = {}) {
  const colorOf = useKindColor();
  const shownKinds = kinds ?? ALL_KINDS;
  const queryClient = useQueryClient();
  const me = useMe().data;
  const myEmail = me?.email ?? null;
  const isAdmin = me?.is_admin ?? false;
  // Deleting is offered to whoever wrote the thing, and to admins; the server checks again.
  const canDelete = (author: string | null | undefined) => isAdmin || (!!author && author === myEmail);
  const vaultsQuery = useVaults();
  const myVaults = vaultsQuery.data ?? [];
  const isPhone = useSyncExternalStore(subscribeWidth, readPhone, () => false);
  // The graph endpoint reads one space at a time (no "all my spaces"). People & topics shares its
  // space with the List and Memory map views (see graph/space.ts); Code map (`kinds` given) opens on Personal.
  const shared = useSharedSpace(!kinds);
  const [codeVault, setCodeVault] = useState<string | undefined>(undefined);
  const vaultId: string | undefined = kinds ? codeVault : shared.vaultId;
  const defaultReady = kinds ? true : shared.ready;
  const graphQuery = useEntityGraph({ kinds, vaultId, enabled: defaultReady && mode === "graph" });
  const listQuery = useQuery({ ...entityListQuery(undefined, vaultId, LIST_LIMIT), enabled: defaultReady && mode === "list", placeholderData: keepPreviousData });
  // The list has no relations, so it stands in for the graph's nodes (find, "Relate to", names).
  const graph: EntityGraphData | null =
    mode === "graph"
      ? (graphQuery.data ?? null)
      : listQuery.data
        ? { nodes: listQuery.data.results.map((e) => ({ id: e.id, kind: e.kind, name: e.name, owner_email: null })), edges: [] }
        : null;
  const activeQuery = mode === "graph" ? graphQuery : listQuery;
  const error =
    activeQuery.error && !graph ? failure(activeQuery.error, "Could not load your people and topics.") : null;
  function chooseVault(id: string | undefined) {
    if (kinds) setCodeVault(id);
    else chooseSpace(id);
  }
  const createEntityMutation = useCreateEntityMutation();
  const updateEntityMutation = useUpdateEntity();
  const deleteEntityMutation = useDeleteEntity();
  const addMemoryMutation = useAddMemory();
  const deleteMemoryMutation = useDeleteMemory();
  const addRelationMutation = useAddRelation();
  const [selected, setSelected] = useState<EntityDetail | null>(null);
  // hidden rather than visible, so a custom kind that shows up later is on by default
  const [hiddenKinds, setHiddenKinds] = useState<Set<EntityKind>>(() => new Set(kinds ? [] : CODE_KINDS));
  const [showCreate, setShowCreate] = useState(false);
  const [createForm, setCreateForm] = useState<{ kind: EntityKind; name: string; aliases: string }>({ kind: shownKinds[0], name: "", aliases: "" });
  const [createError, setCreateError] = useState<Failure | null>(null);
  const [creating, setCreating] = useState(false);

  const [editing, setEditing] = useState(false);
  const [editForm, setEditForm] = useState({ name: "", aliases: "", summary: "" });
  const [memoryText, setMemoryText] = useState("");
  const [relForm, setRelForm] = useState({ to: "", label: "" });
  // the add forms stay folded behind a text button, so the inspector opens as something to read
  const [adding, setAdding] = useState<"memory" | "relation" | null>(null);
  const [actionError, setActionError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  // "Add memory" progress: spinner while saving, a drawn check for 1.2s after
  const [memoryStatus, setMemoryStatus] = useState<"idle" | "running" | "done">("idle");
  const [freshMemory, setFreshMemory] = useState<string | null>(null);
  // the memory a search result pointed at (`?memory=`): tinted until the inspector moves on
  const [pointedMemory, setPointedMemory] = useState<string | null>(null);
  const memoryTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(memoryTimer.current), []);

  // px of canvas covered by the floating header/toolbar (top) and, on desktop,
  // the inspector column (right), so the camera fits nodes into what is left.
  const chromeRef = useRef<HTMLDivElement>(null);
  // "Hide controls" leaves the whole canvas to the graph; remembered in this browser
  const chromeHidden = useSyncExternalStore(subscribeChrome, readChromeHidden, () => false);
  function toggleChrome() {
    try {
      window.localStorage.setItem(CHROME_KEY, chromeHidden ? "0" : "1");
    } catch {
      chromeFallback = !chromeHidden;
    }
    chromeListeners.forEach((l) => l());
  }

  const [insets, setInsets] = useState<CanvasInsets>({ top: 0, right: 0, bottom: 0, left: 0 });
  const inspectorOpen = !!selected;
  const hasGraph = !!graph && graph.nodes.length > 0;
  useEffect(() => {
    const el = chromeRef.current;
    const measure = () => {
      const md = window.matchMedia("(min-width: 768px)").matches;
      // below md the controls sit above the canvas in the page flow, so they cover none of it,
      // and the inspector is a full-screen sheet; on desktop it covers the right column only while open
      const top = md && el ? el.offsetTop + el.offsetHeight : 0;
      setInsets((prev) => {
        const next = { top, right: md && inspectorOpen ? INSPECTOR_RESERVE : 0, bottom: md ? 48 : 64, left: 0 };
        return prev.top === next.top && prev.right === next.right && prev.bottom === next.bottom ? prev : next;
      });
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (el) ro.observe(el);
    window.addEventListener("resize", measure);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [hasGraph, chromeHidden, inspectorOpen, mode]);

  function toggleKind(kind: EntityKind) {
    setHiddenKinds((prev) => {
      const next = new Set(prev);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }
  // the one "Code" chip: all three code kinds on or off together
  function toggleCode(show: boolean) {
    setHiddenKinds((prev) => {
      const next = new Set(prev);
      CODE_KINDS.forEach((k) => (show ? next.delete(k) : next.add(k)));
      return next;
    });
  }
  function reveal(kind: EntityKind) {
    if (!kinds && CODE_KINDS.includes(kind)) toggleCode(true);
    else
      setHiddenKinds((prev) => {
        const next = new Set(prev);
        next.delete(kind);
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
    setAdding(null);
    setActionError(null);
    try {
      const detail = await queryClient.fetchQuery(entityDetailQuery(id));
      setSelected(detail);
      return detail;
    } catch {
      setSelected(null);
    }
  }

  // Deep links: `?id=<entity>` opens its inspector in its own space, `?vault=<space>` picks a space.
  // The open entity is written back to the URL, so an inspector view can be shared or bookmarked.
  const urlIdRef = useRef<string | null>(null); // the id the URL and the inspector last agreed on
  const urlVaultRef = useRef<string | null>(null);
  const focusSeq = useRef(0);
  const [pendingFocus, setPendingFocus] = useState<{ id: string | null; vault: string | null } | null>(null);
  const toGraphVault = (id: string | null | undefined): string | undefined => {
    if (!id) return undefined;
    return myVaults.find((v) => v.id === id)?.is_default ? undefined : id;
  };
  useEffect(() => {
    if (!pendingFocus || !vaultsQuery.isFetched) return;
    const { id, vault } = pendingFocus;
    // No need to clear pendingFocus: the refs below make each URL value act once. UrlFocus remounts when the page swaps loading/empty/graph layouts: act on each URL value once
    const seq = ++focusSeq.current; // a newer link wins over one still resolving
    void (async () => {
      await Promise.resolve(); // state changes happen after the effect, not inside it
      if (seq !== focusSeq.current) return;
      if (vault !== urlVaultRef.current) {
        urlVaultRef.current = vault;
        if (vault && myVaults.some((v) => v.id === vault)) chooseVault(toGraphVault(vault));
      }
      if (!id || id === urlIdRef.current) return;
      urlIdRef.current = id;
      const home = vault ?? (await findEntityVault(queryClient, id));
      if (seq !== focusSeq.current) return;
      if (home && myVaults.some((v) => v.id === home)) chooseVault(toGraphVault(home));
      const detail = await selectNode(id);
      if (detail && seq === focusSeq.current) reveal(detail.kind);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingFocus, vaultsQuery.isFetched]);
  useEffect(() => {
    const id = selected?.id ?? null;
    if (id === urlIdRef.current) return;
    urlIdRef.current = id;
    const url = new URL(window.location.href);
    if (id) url.searchParams.set("id", id);
    else url.searchParams.delete("id");
    url.searchParams.delete("vault");
    url.searchParams.delete("memory");
    window.history.replaceState(null, "", url.pathname + url.search);
  }, [selected?.id]);

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
      const targetVault = vaultId ?? myVaults.find((v) => v.is_default)?.id;
      if (!targetVault) throw new Error("Your Personal space could not be loaded. Try again.");
      const created = await createEntityMutation.mutateAsync({
        kind: createForm.kind,
        name,
        aliases: aliases.length ? aliases : undefined,
        vault_id: targetVault,
      });
      setShowCreate(false);
      setCreateForm({ kind: shownKinds[0], name: "", aliases: "" });
      await selectNode(created.id);
    } catch (err) {
      setCreateError(failure(err, "Could not add that person or topic. Check the name and try again."));
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
      setActionError(failure(err, "Could not delete this person or topic. Try again."));
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
      const detail = await selectNode(selected.id); // also folds the form again
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

  // Graph, desktop: everything floats over one full-bleed canvas. Header and toolbar run the full
  // content width at the same page inset as every other route (main's px-4 py-6, sm px-8, lg px-12
  // py-38) and give up the right column only while the inspector is open; zoom and "Hide controls"
  // sit bottom right (inside GraphCanvas). Below md the header and toolbar sit in the page flow above
  // the canvas instead, so nothing covers the title or the graph.
  // List: header and toolbar on top, the table scrolling below, the inspector in the same place.
  // `alwaysShow`: states with no canvas (loading, error, empty) have nowhere to put "Show controls".
  const frame = (toolbar: ReactNode, content: ReactNode, inspector?: ReactNode, aside?: ReactNode, alwaysShow = false) => {
    const list = mode === "list";
    const showChrome = list || alwaysShow || !chromeHidden;
    // inline, so it wins over the responsive page padding
    const reserve = inspector && !isPhone ? { paddingRight: "25rem" } : undefined;
    return (
      <div className="absolute inset-0 overflow-hidden flex flex-col md:block">
        <Suspense fallback={null}>
          <UrlFocus
            onChange={(id, vault, memory) => {
              setPendingFocus({ id, vault });
              setPointedMemory(memory);
            }}
          />
        </Suspense>
        {list ? (
          <div className="absolute inset-0 flex flex-col">
            <div ref={chromeRef} className="shrink-0 px-4 pt-6 sm:px-8 lg:px-12 lg:pt-[38px] pb-3 flex flex-col gap-3 items-start w-full" style={reserve}>
              {header}
              {toolbar}
            </div>
            <div className="flex-1 min-h-0 overflow-y-auto px-4 sm:px-8 lg:px-12 pb-8" style={reserve}>
              {content}
            </div>
          </div>
        ) : (
          <>
            {showChrome && (
              <div
                ref={chromeRef}
                className={"relative shrink-0 md:absolute md:left-0 md:top-0 md:z-10 px-4 pt-6 sm:px-8 lg:px-12 lg:pt-[38px] pb-3 flex flex-col gap-3 items-start w-full md:pointer-events-none md:select-none md:[&>*]:pointer-events-auto md:[background:linear-gradient(var(--canvas)_85%,transparent)]"}
                style={reserve}
              >
                {header}
                {toolbar}
                {aside}
              </div>
            )}
            <div className="relative flex-1 min-h-0 md:absolute md:inset-0">{content}</div>
          </>
        )}
        {inspector}
      </div>
    );
  };

  if (error) {
    return frame(
      null,
      <div className="h-full flex items-center justify-center p-4">
        <div className="max-w-sm flex flex-col gap-2">
          <ErrorLine error={error} />
          <button type="button" className="btn btn-primary btn-sm self-start" onClick={() => window.location.reload()}>
            Try again
          </button>
        </div>
      </div>,
      undefined,
      undefined,
      true
    );
  }

  if (!graph) {
    return frame(
      <div className="flex gap-2" aria-busy="true" aria-label="Loading people and topics">
        {[64, 92, 80, 112].map((w) => (
          <span key={w} className="skeleton h-6" style={{ width: w }} />
        ))}
      </div>,
      mode === "list" ? (
        <div className="flex flex-col gap-3 pt-2" aria-hidden>
          {[0, 1, 2, 3, 4, 5].map((i) => (
            <span key={i} className="skeleton h-10 w-full" />
          ))}
        </div>
      ) : (
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
      ),
      undefined,
      undefined,
      true
    );
  }

  // the full graph (no `kinds` given) also carries agents' custom kinds: one filter each
  const filterKinds = kinds
    ? shownKinds
    : [...shownKinds, ...[...new Set(graph.nodes.map((n) => n.kind))].filter((k) => !ALL_KINDS.includes(k)).sort()];
  const visibleKinds = new Set(filterKinds.filter((k) => !hiddenKinds.has(k)));
  const counts = new Map<EntityKind, number>();
  graph.nodes.forEach((n) => counts.set(n.kind, (counts.get(n.kind) ?? 0) + 1));
  // People & topics folds Code map's kinds into one "Code" chip, shown only when there is code to show
  const groupCode = !kinds;
  const codeCount = CODE_KINDS.reduce((sum, k) => sum + (counts.get(k) ?? 0), 0);
  const codeShown = CODE_KINDS.some((k) => visibleKinds.has(k));
  const chipKinds = groupCode ? filterKinds.filter((k) => !CODE_KINDS.includes(k)) : filterKinds;
  const createKinds = groupCode ? filterKinds.filter((k) => !CODE_KINDS.includes(k)) : filterKinds;
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
        className="bauhaus-dialog pop-in p-6 w-full max-w-md flex flex-col gap-4"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-entity-title"
      >
        <div className="flex items-center justify-between">
          <h2 id="new-entity-title" className="text-[20px] font-semibold tracking-[-0.01em]">
            Add a person or topic
          </h2>
          <button type="button" className="btn btn-ghost btn-icon" onClick={() => setShowCreate(false)} aria-label="Close">
            <XIcon {...ICON} />
          </button>
        </div>
        <p className="text-[14px] -mt-1" style={{ color: "var(--ink-dim)" }}>
          Your AI apps also add people and topics on their own as they learn. Add one here to start saving memories about it.
        </p>
        <div className="text-[14px] font-medium flex flex-col gap-1.5">
          Kind
          <Select
            aria-label="Kind"
            className="h-10 text-[14px] w-full"
            value={createForm.kind}
            onChange={(v) => setCreateForm((f) => ({ ...f, kind: v as EntityKind }))}
            options={createKinds.map((k) => ({ value: k, label: kindLabel(k) }))}
          />
        </div>
        <label className="text-[14px] font-medium flex flex-col gap-1.5">
          Name
          <input
            autoFocus
            className="field h-10 px-3 text-[14px] font-normal"
            value={createForm.name}
            onChange={(e) => setCreateForm((f) => ({ ...f, name: e.target.value }))}
            placeholder="e.g. Jordan Blake"
          />
        </label>
        <label className="text-[14px] font-medium flex flex-col gap-1.5">
          Other names (optional, separate with commas)
          <input
            className="field h-10 px-3 text-[14px] font-normal"
            value={createForm.aliases}
            onChange={(e) => setCreateForm((f) => ({ ...f, aliases: e.target.value }))}
            placeholder="e.g. JB, Jordy"
          />
        </label>
        {createError && <ErrorLine error={createError} />}
        <div className="flex justify-end gap-2 pt-1">
          <button type="button" className="btn" onClick={() => setShowCreate(false)}>
            Cancel
          </button>
          <button type="submit" disabled={creating || !createForm.name.trim()} className="btn btn-primary">
            {creating ? "Adding…" : "Add"}
          </button>
        </div>
      </form>
    </div>
  );

  const vaultSwitcher = (
    <SpaceSelect
      className="h-10 text-[14px] flex-1 min-w-0 md:flex-none md:w-56"
      value={vaultId}
      onChange={(v) => {
        chooseVault(v);
        setSelected(null);
      }}
    />
  );

  const chip = (key: string, label: string, color: string, count: number, active: boolean, onClick: () => void) => (
    <button key={key} type="button" className="pill shrink-0" aria-pressed={active} onClick={onClick}>
      <span className="w-2 h-2 rounded-full shrink-0" style={{ background: color, opacity: active ? 1 : 0.35 }} aria-hidden />
      {label}
      <span className="tabular-nums" style={{ color: "var(--ink-dim)" }}>
        {count}
      </span>
      <span className="sr-only">{active ? "(shown)" : "(hidden)"}</span>
    </button>
  );

  // Desktop: space, find and add on one row, then the kind chips. Phones: space and add, then find on
  // its own full-width row, then the chips in one row that scrolls sideways out to the screen edges.
  const toolbar = (
    <div className="flex flex-col gap-2 w-full min-w-0">
      <div className="flex flex-wrap md:flex-nowrap items-center gap-2 w-full min-w-0">
        {vaultSwitcher}
        {graph.nodes.length > 0 && (
          <form
            className="order-last basis-full md:order-none md:basis-auto min-w-0"
            onSubmit={(e) => {
              e.preventDefault();
              const input = e.currentTarget.elements.namedItem("find") as HTMLInputElement;
              const q = input.value.trim().toLowerCase();
              const hit = graph.nodes.find((n) => n.name.toLowerCase() === q) ?? graph.nodes.find((n) => n.name.toLowerCase().includes(q));
              if (q && hit) {
                reveal(hit.kind);
                selectNode(hit.id);
                input.value = "";
              }
            }}
          >
            <input
              name="find"
              data-page-search
              list="entity-names"
              className="field h-10 w-full md:w-56 px-3 text-[14px]"
              placeholder="Find a person or topic…"
              aria-label="Find a person or topic"
              autoComplete="off"
            />
            <datalist id="entity-names">
              {graph.nodes.map((n) => (
                <option key={n.id} value={n.name} />
              ))}
            </datalist>
          </form>
        )}
        {!kinds && (
          // Code map is filled by coding apps only: nothing to add by hand there
          // Most people come here to read: adding is a secondary action for members, primary for admins.
          <button
            type="button"
            className={`btn ${isAdmin ? "btn-primary" : ""} shrink-0 ml-auto md:ml-0`}
            aria-label="Add person or topic"
            onClick={() => setShowCreate(true)}
          >
            <PlusIcon weight="duotone" size={18} aria-hidden />
            {/* phones keep a short label rather than a bare + */}
            <span className="sm:hidden" aria-hidden>
              Add
            </span>
            <span className="max-sm:hidden">Add person or topic</span>
          </button>
        )}
      </div>
      {/* a chip only for a kind that has something in it; no row at all while everything is empty */}
      {graph.nodes.length > 0 && (
        <div
          className="flex gap-2 items-center max-md:flex-nowrap max-md:overflow-x-auto max-md:[scrollbar-width:none] max-md:py-1 max-sm:-mx-4 max-sm:px-4 sm:max-md:-mx-8 sm:max-md:px-8 md:flex-wrap"
          role="group"
          aria-label="Show kinds"
        >
          {chipKinds
            .filter((kind) => (counts.get(kind) ?? 0) > 0)
            .map((kind) => chip(kind, kindLabel(kind), colorOf(kind), counts.get(kind) ?? 0, visibleKinds.has(kind), () => toggleKind(kind)))}
          {groupCode && codeCount > 0 && chip("code", "Code", colorOf("repository"), codeCount, codeShown, () => toggleCode(!codeShown))}
        </div>
      )}
    </div>
  );

  if (graph.nodes.length === 0) {
    // the empty message sits right under the toolbar, where the first row would be, not mid-canvas
    const empty = guide ? (
      <EmptyState title="Nothing mapped yet." action={guide.action}>
        {guide.text}
      </EmptyState>
    ) : (
      <EmptyState title="No people or topics yet.">
        {vaultId
          ? "People and topics appear here as your team's AI apps save memories in this space, or you can add one yourself."
          : "They appear as your connected sources sync and your AI apps save memories, or you can add one yourself."}
      </EmptyState>
    );
    return (
      <>
        {mode === "list" ? frame(toolbar, empty, undefined, undefined, true) : frame(toolbar, null, undefined, empty, true)}
        {createModal}
      </>
    );
  }

  const lastTouched = selected
    ? selected.memory.reduce<string | undefined>((a, m) => (m.created_at && (!a || m.created_at > a) ? m.created_at : a), undefined)
    : undefined;
  // One author for every memory: say so once at the top, not on each row.
  const soleAuthor =
    selected && selected.memory.length > 0 && selected.memory.every((m) => m.owner_email === selected.memory[0].owner_email)
      ? selected.memory[0].owner_email
      : undefined;
  const topAuthor = soleAuthor ?? selected?.owner_email;

  const inspector = selected && (
    <aside
      aria-label={`${selected.name} details`}
      // phones: a full-screen sheet with Back; desktop: a card in the right column
      className={
        isPhone
          ? "panel fixed inset-0 z-50 flex flex-col"
          : "panel pop-in absolute z-20 flex flex-col right-6 top-6 w-[22rem] max-h-[calc(100%-5rem)]"
      }
      style={
        isPhone
          ? { borderRadius: 0, border: 0 }
          : { transformOrigin: "top right" }
      }
    >
      {isPhone && (
        <div className="shrink-0 px-2 pt-2" style={{ borderBottom: "1px solid var(--border)" }}>
          <button type="button" className="btn btn-ghost btn-sm mb-2" onClick={() => setSelected(null)}>
            <ArrowLeftIcon weight="duotone" size={16} aria-hidden />
            Back
          </button>
        </div>
      )}
      <div className="flex items-start justify-between gap-2 p-5 pb-3">
        <div className="flex items-center gap-3 min-w-0">
          {selected.kind === "person" && <Monogram name={selected.name || selected.id} size={36} color={colorOf("person")} />}
          <div className="min-w-0">
            <h2 className="text-[20px] font-medium leading-tight tracking-[-0.015em] truncate">{selected.name}</h2>
            <span className="text-[13px] inline-flex items-center gap-1.5 mt-0.5" style={{ color: "var(--ink-dim)" }}>
              <span className="w-2 h-2 rounded-full" style={{ background: kindColor(selected.kind) }} aria-hidden />
              {kindLabel(selected.kind)}
              {isCustomKind(selected.kind) && canDelete(selected.owner_email) && (
                // the dot doubles as the colour picker for an agent's own kind (for whoever can edit it)
                <input
                  type="color"
                  aria-label={`Colour for ${kindLabel(selected.kind)}`}
                  title="Change this kind's colour"
                  className="kind-swatch"
                  value={/^#[0-9a-f]{6}$/i.test(kindColor(selected.kind)) ? kindColor(selected.kind) : defaultKindHex(selected.kind)}
                  onChange={(e) => setKindColor(selected.kind, e.target.value)}
                />
              )}
            </span>
          </div>
        </div>
        <TooltipGroup>
          <div className="flex items-center gap-0.5 shrink-0 -mr-1">
            <Tooltip label="Ask what this affects">
              <Link
                href={`/chat?ask=${encodeURIComponent(`If something changes with ${selected.name}, what does it affect?`)}`}
                className="btn btn-ghost btn-sm btn-icon"
                aria-label="Ask what this affects"
              >
                <LightningIcon {...ICON} />
              </Link>
            </Tooltip>
            {canDelete(selected.owner_email) && !editing && (
              <Tooltip label="Edit">
                <button className="btn btn-ghost btn-sm btn-icon" onClick={startEditing} aria-label="Edit">
                  <PencilSimpleIcon {...ICON} />
                </button>
              </Tooltip>
            )}
            {canDelete(selected.owner_email) && (
              <Tooltip label="Delete. Also removes its memories and connections">
                <ConfirmButton label="Delete" confirmLabel="Delete" onConfirm={deleteSelected} disabled={busy} className="btn btn-ghost btn-sm btn-icon">
                  <TrashIcon {...ICON} />
                </ConfirmButton>
              </Tooltip>
            )}
            {!isPhone && (
              <Tooltip label="Close" shortcut="Esc">
                <button className="btn btn-ghost btn-sm btn-icon" onClick={() => setSelected(null)} aria-label="Close">
                  <XIcon {...ICON} />
                </button>
              </Tooltip>
            )}
          </div>
        </TooltipGroup>
      </div>

      <div className="overflow-y-auto px-5 pb-5 flex flex-col gap-6">
        <div className="flex flex-col gap-1">
          {topAuthor && <Byline email={topAuthor} />}
          {lastTouched && (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              Last memory {ago(lastTouched)}
            </p>
          )}
          {selected.aliases.length > 0 && (
            <p className="text-[13px] truncate" style={{ color: "var(--ink-dim)" }} title={selected.aliases.join(", ")}>
              Also called {selected.aliases.join(", ")}
            </p>
          )}
        </div>

        {actionError && <ErrorLine error={actionError} />}

        {editing ? (
          <form onSubmit={saveEdit} className="flex flex-col gap-2.5">
            <label className="text-[13px] font-medium flex flex-col gap-1">
              Name
              <input className="field h-10 px-3 text-[14px] font-normal" value={editForm.name} onChange={(e) => setEditForm((f) => ({ ...f, name: e.target.value }))} />
            </label>
            <label className="text-[13px] font-medium flex flex-col gap-1">
              Other names (separate with commas)
              <input className="field h-10 px-3 text-[14px] font-normal" value={editForm.aliases} onChange={(e) => setEditForm((f) => ({ ...f, aliases: e.target.value }))} />
            </label>
            <label className="text-[13px] font-medium flex flex-col gap-1">
              Summary
              <textarea
                className="field px-3 py-2 text-[14px] font-normal"
                rows={3}
                value={editForm.summary}
                onChange={(e) => setEditForm((f) => ({ ...f, summary: e.target.value }))}
              />
            </label>
            <div className="flex gap-2">
              <button type="submit" disabled={busy} className="btn btn-primary btn-sm">
                Save
              </button>
              <button type="button" className="btn btn-sm" onClick={() => setEditing(false)}>
                Cancel
              </button>
            </div>
          </form>
        ) : (
          selected.summary && (
            <p className="text-[14px] leading-relaxed -mt-2" style={{ color: "var(--ink-dim)" }}>
              {selected.summary}
            </p>
          )
        )}

        <PropertyValues properties={selected.properties} />

        <section className="flex flex-col gap-2">
          <h3 className="section-title flex items-baseline gap-2">
            Memory
            <span className="font-mono text-[12px] font-normal" style={{ color: "var(--ink-faint)" }}>
              {selected.memory.length}
            </span>
          </h3>
          {selected.memory.length === 0 ? (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              Nothing saved yet.
            </p>
          ) : (
            <ul className="hairline-rows">
              {selected.memory.map((m) => (
                <li
                  key={m.id}
                  ref={m.id === freshMemory ? blurIn : m.id === pointedMemory ? scrollToMemory : undefined}
                  className="group py-3 flex items-start justify-between gap-2"
                  style={m.id === pointedMemory ? { background: "var(--accent-soft)", boxShadow: "-8px 0 0 var(--accent-soft), 8px 0 0 var(--accent-soft)" } : undefined}
                  aria-current={m.id === pointedMemory ? "true" : undefined}
                >
                  <div className="min-w-0 flex flex-col gap-1">
                    {/* edit and delete follow one rule: whoever wrote it, and admins */}
                    {canDelete(m.owner_email) ? (
                      <EditableMemoryText memory={m} disabled={busy} onSaved={() => void selectNode(selected.id)} />
                    ) : (
                      <span className="text-[14px] leading-normal">{m.text}</span>
                    )}
                    <Byline email={soleAuthor !== undefined ? null : m.owner_email} at={m.created_at} />
                  </div>
                  {canDelete(m.owner_email) && (
                    <Tooltip label="Delete memory">
                      <ConfirmButton
                        label="Delete memory"
                        confirmLabel="Delete"
                        onConfirm={() => deleteMemory(m.id)}
                        disabled={busy}
                        className="btn btn-ghost btn-sm btn-icon shrink-0 -mr-1"
                      >
                        <XIcon weight="duotone" size={16} />
                      </ConfirmButton>
                    </Tooltip>
                  )}
                </li>
              ))}
            </ul>
          )}
          {adding === "memory" ? (
          <form onSubmit={submitMemory} className="flex flex-col gap-2">
            <textarea
              autoFocus
              className="field px-3 py-2 text-[14px]"
              rows={2}
              aria-label={`New memory about ${selected.name}`}
              placeholder="Add a memory about this person or topic…"
              value={memoryText}
              onChange={(e) => setMemoryText(e.target.value)}
            />
            <div className="flex items-center gap-2">
              <button type="submit" disabled={busy || !memoryText.trim()} className="btn btn-sm">
                Add memory
              </button>
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => setAdding(null)}>
                Cancel
              </button>
            </div>
          </form>
          ) : (
            <div className="flex items-center gap-3">
              <button type="button" className="btn btn-ghost btn-sm self-start -ml-3" onClick={() => setAdding("memory")}>
                <PlusIcon weight="duotone" size={16} aria-hidden />
                Add memory
              </button>
              {memoryStatus !== "idle" && (
                <span className="inline-flex items-center gap-1.5 text-[13px]" style={{ color: "var(--ink-dim)" }} role="status">
                  <SyncMark status={memoryStatus} />
                  {memoryStatus === "running" ? "Saving…" : "Saved"}
                </span>
              )}
            </div>
          )}
          {selected.memory.length > 1 && selected.memory.every((m) => canDelete(m.owner_email)) && <TidyUp entityId={selected.id} disabled={busy} onDone={() => void selectNode(selected.id)} />}
        </section>

        <section className="flex flex-col gap-2">
          <h3 className="section-title flex items-baseline gap-2">
            Connections
            <span className="font-mono text-[12px] font-normal" style={{ color: "var(--ink-faint)" }}>
              {selected.relations.length}
            </span>
          </h3>
          {selected.relations.length === 0 ? (
            <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              No connections saved yet.
            </p>
          ) : (
            <ul className="hairline-rows">
              {selected.relations.map((r) => {
                const other = r.direction === "out" ? r.out : r.in;
                return (
                  // each connection reads as a sentence: "works on Billing Rewrite" (outgoing) or
                  // "Tom Becker works on this" (incoming), never a bare arrow
                  <li key={r.id} className="py-2.5 flex items-baseline gap-1.5 min-w-0 text-[14px]" title={r.owner_email ? `Added by ${r.owner_email}` : undefined}>
                    {r.direction === "out" ? (
                      <>
                        <span className="shrink-0" style={{ color: "var(--ink-dim)" }}>
                          {r.label.replace(/_/g, " ")}
                        </span>
                        <button type="button" className="link truncate text-left min-w-0" onClick={() => selectNode(other)}>
                          {nameOf(other)}
                        </button>
                      </>
                    ) : (
                      <>
                        <button type="button" className="link truncate text-left min-w-0" onClick={() => selectNode(other)}>
                          {nameOf(other)}
                        </button>
                        <span className="shrink-0" style={{ color: "var(--ink-dim)" }}>
                          {r.label.replace(/_/g, " ")} this
                        </span>
                      </>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {adding === "relation" ? (
          <form onSubmit={submitRelation} className="flex flex-col gap-2">
            <Select
              aria-label="Relate to"
              placeholder="Relate to…"
              className="h-10 text-[14px] w-full"
              value={relForm.to}
              onChange={(v) => setRelForm((f) => ({ ...f, to: v }))}
              options={graph.nodes
                .filter((n) => n.id !== selected.id)
                .map((n) => ({ value: n.id, label: n.name, hint: kindLabel(n.kind) }))}
            />
            <input
              className="field h-10 px-3 text-[14px]"
              placeholder="How they connect, e.g. works at"
              aria-label="Relation label"
              value={relForm.label}
              onChange={(e) => setRelForm((f) => ({ ...f, label: e.target.value }))}
            />
            <div className="flex gap-2">
              <button type="submit" disabled={busy || !relForm.to || !relForm.label.trim()} className="btn btn-sm">
                Add connection
              </button>
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => setAdding(null)}>
                Cancel
              </button>
            </div>
          </form>
          ) : (
            graph.nodes.length > 1 && (
              <button type="button" className="btn btn-ghost btn-sm self-start -ml-3" onClick={() => setAdding("relation")}>
                <PlusIcon weight="duotone" size={16} aria-hidden />
                Add connection
              </button>
            )
          )}
        </section>
      </div>
    </aside>
  );

  return (
    <>
      {frame(
        toolbar,
        mode === "list" ? (
          <EntityList
            entities={(listQuery.data?.results ?? []).filter((e) => visibleKinds.has(e.kind))}
            total={listQuery.data?.has_more ? listQuery.data.total : undefined}
            kindLabel={kindLabel}
            kindColor={colorOf}
            selectedId={selected?.id}
            onSelect={selectNode}
          />
        ) : (
          <GraphCanvas
            nodes={graph.nodes}
            edges={graph.edges}
            kindColor={colorOf}
            visibleKinds={visibleKinds}
            insets={insets}
            selectedId={selected?.id}
            onSelect={selectNode}
            myEmail={myEmail}
            ariaLabel="Entity relationship graph"
            chrome={{ hidden: chromeHidden, toggle: toggleChrome }}
          />
        ),
        inspector,
        mode === "graph" && guide && graph.nodes.length < SPARSE && (
          <div className="max-w-sm text-[13px] flex flex-col gap-2 items-start" style={{ color: "var(--ink-dim)" }}>
            <p>{guide.text}</p>
            {guide.action}
          </div>
        )
      )}
      {createModal}
    </>
  );
}
