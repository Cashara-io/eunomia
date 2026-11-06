"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { ArrowUpRight, Braces, Building2, CircleDot, FileCode, FileText, FolderGit2, FolderKanban, KeyRound, MapPin, Plug, Plus, Search, Settings, Upload, User, X } from "lucide-react";
import Shortcuts from "./Shortcuts";
import { ADVANCED_NAV, MAIN_NAV } from "./Sidebar";
import { documentHref, type EntityKind } from "@/lib/types";
import { useSources } from "@/lib/queries/sources";
import { usePaletteSearch } from "@/lib/queries/tools";
import { useMe } from "@/lib/queries/auth";
import { useVaults } from "@/lib/queries/vaults";
import { modKey } from "@/lib/platform";
import { SETTINGS_GROUPS } from "@/lib/settingsTabs";

type IconType = React.ComponentType<{ size?: number; color?: string; strokeWidth?: number }>;
type Item = { key: string; label: string; sub?: string; icon: IconType; go: () => void; score?: number };
type Group = { title: string; items: Item[] };

const OPEN_EVENT = "eunomia:open-palette";
/** Opens the palette from anywhere (the sidebar search button, page CTAs). */
export function openCommandPalette() {
  window.dispatchEvent(new Event(OPEN_EVENT));
}

// One icon per kind: only a person gets the person icon. An agent's own kind ("project", "decision")
// gets a neutral dot, or a folder for a project.
const ENTITY_ICON: Record<string, IconType> = {
  person: User,
  organisation: Building2,
  location: MapPin,
  repository: FolderGit2,
  file: FileCode,
  symbol: Braces,
  project: FolderKanban,
};
const entityIcon = (kind: string): IconType => ENTITY_ICON[kind.toLowerCase()] ?? CircleDot;
// "project" -> "Project", "open_question" -> "Open question"
const kindWord = (kind: string) => {
  const builtIn = KIND_WORD[kind];
  if (builtIn) return builtIn;
  const words = kind.replace(/[_-]+/g, " ").trim().toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
};

// Pages only admins use; hidden from everyone else (the sidebar hides them too).
const ADMIN_HREFS = new Set(["/code", "/skill"]);

// Pages, with the sidebar's own labels. Read when the palette renders (not at
// module load), since Sidebar imports this module too.
const staticNav = (): { label: string; href: string; icon: IconType }[] => [...MAIN_NAV, ...ADVANCED_NAV];

const KIND_WORD: Record<string, string> = {
  person: "Person",
  organisation: "Organisation",
  location: "Place",
  repository: "Repository",
  file: "File",
  symbol: "Function",
};

// Settings tabs come from the one list the Settings page also reads; admin groups are for admins only.
type SettingsEntry = { id: string; label: string; group: string; admin: boolean; words: string };
const SETTINGS_ENTRIES: SettingsEntry[] = SETTINGS_GROUPS.flatMap((g) =>
  g.tabs.map((t) => ({ id: t.id, label: t.label, group: g.label, admin: g.admin, words: t.keywords }))
);

/** How well `label` (plus extra `words`) matches `needle`: 0 is best, null is no match. Exact and
 * prefix matches on the label beat a keyword hit, which beats a match in the middle of a word. */
function matchScore(needle: string, label: string, words = ""): number | null {
  const l = label.toLowerCase();
  if (!needle) return 3;
  if (l === needle) return 0;
  if (l.startsWith(needle)) return 1;
  if (l.split(/[^a-z0-9]+/).some((w) => w.startsWith(needle))) return 2;
  if (words.toLowerCase().split(/\s+/).some((w) => w.startsWith(needle))) return 2;
  if (`${l} ${words.toLowerCase()}`.includes(needle)) return 3;
  return null;
}

// Things to do, each found by its label or a few plain words people might type instead.
const ACTIONS: { label: string; href: string; icon: IconType; words: string; admin?: boolean }[] = [
  { label: "Create a memory space", href: "/vaults", icon: Plus, words: "new space vault team share" },
  { label: "Upload a document", href: "/documents", icon: Upload, words: "add file pdf import" },
  { label: "New API token", href: "/settings?tab=tokens", icon: KeyRound, words: "create key access token" },
  { label: "Connect an AI app", href: "/", icon: Plug, words: "claude chatgpt cursor codex mcp setup" },
];

const CODE_KINDS = new Set<EntityKind>(["repository", "file", "symbol"]);
const SEARCH_DEBOUNCE_MS = 250;
const SEARCH_MIN_LENGTH = 2;
const SEARCH_LIMIT = 5;

export default function CommandPalette() {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setOpen((v) => !v);
      } else if (e.key === "Escape") {
        setOpen(false);
      }
    }
    const onOpen = () => setOpen(true);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener(OPEN_EVENT, onOpen);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener(OPEN_EVENT, onOpen);
    };
  }, []);

  // Mounted only while open, so every open starts from fresh state. No
  // open/close animation: this is opened from the keyboard many times a day.
  return (
    <>
      <Shortcuts paletteOpen={open} openPalette={() => setOpen(true)} />
      {open && <Palette onClose={() => setOpen(false)} />}
    </>
  );
}

function Palette({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const rows = useSources().data ?? [];
  const [debounced, setDebounced] = useState("");
  const router = useRouter();
  const isAdmin = useMe().data?.is_admin ?? false;
  const vaults = useVaults().data ?? [];
  const spaceName = (id?: string) => {
    const v = id ? vaults.find((x) => x.id === id) : undefined;
    return v ? (v.kind === "personal" ? "Personal" : v.name) : null;
  };

  // Data search, debounced -- only fires once the nav/connector match is
  // worth supplementing, i.e. the query is long enough to be meaningful.
  const q = query.trim();
  const searching = q.length >= SEARCH_MIN_LENGTH;
  useEffect(() => {
    if (!searching) return;
    const timer = setTimeout(() => setDebounced(q), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [q, searching]);
  const { entities: entityHits, records: recordHits } = usePaletteSearch(debounced, SEARCH_LIMIT, debounced !== "");

  const connectorItems: Item[] = rows
    .filter((r) => r.connected)
    .map((r) => ({ key: `conn:${r.key}`, label: r.label, icon: Plug, go: () => router.push(`/connectors/${r.key}`) }));
  const staticItems: Item[] = staticNav()
    .filter((s) => isAdmin || !ADMIN_HREFS.has(s.href))
    .map((s) => ({
    key: `nav:${s.href}`,
    label: s.label,
    icon: s.icon,
    go: () => router.push(s.href),
  }));
  const needle = query.trim().toLowerCase();
  // best match first within each group; a stable sort keeps the sidebar's order among equals
  const ranked = (list: Item[]) => list.filter((i) => i.score !== undefined).sort((a, b) => a.score! - b.score!);
  const scored = (item: Item, words?: string): Item => ({ ...item, score: matchScore(needle, item.label, words) ?? undefined });
  const navItems = ranked([...staticItems, ...connectorItems].map((item) => scored(item)));
  // Settings tabs only once something is typed, so the empty palette stays short.
  const settingsItems: Item[] = needle
    ? ranked(
        SETTINGS_ENTRIES.filter((t) => !t.admin || isAdmin).map((t) =>
          scored(
            {
              key: `settings:${t.id}`,
              label: t.label,
              sub: `Settings › ${t.group}`,
              icon: Settings,
              go: () => router.push(`/settings?tab=${t.id}`),
            },
            `${t.words} settings`
          )
        )
      )
    : [];
  const actionItems: Item[] = ranked(
    ACTIONS.map((a) =>
      scored({ key: `action:${a.label}`, label: a.label, icon: a.icon, go: () => router.push(a.href) }, a.words)
    )
  );

  const entityItems: Item[] = (searching ? entityHits : []).map((e) => ({
    key: `entity:${e.id}`,
    label: e.name,
    // the space tells apart two "Priya Nair"s from different spaces
    sub: [kindWord(e.kind), spaceName((e as { vault?: string }).vault)].filter(Boolean).join(" · "),
    icon: entityIcon(e.kind),
    // opens the entity's inspector, in its own space (see EntityGraph's deep links)
    go: () => {
      const q = new URLSearchParams({ id: e.id });
      const vault = (e as { vault?: string }).vault; // entities_search returns each hit's space
      if (vault) q.set("vault", vault);
      router.push(`${CODE_KINDS.has(e.kind) ? "/code" : "/entities"}?${q}`);
    },
  }));
  const recordItems: Item[] = (searching ? recordHits : []).map((h) => ({
    key: `record:${h.id}`,
    label: h.title || h.snippet,
    sub: h.document ? h.document.filename : h.source,
    icon: FileText,
    go: () => {
      // a passage of an uploaded document opens that document at the passage
      if (h.document) {
        router.push(documentHref(h.document));
      } else if (h.url) {
        window.open(h.url, "_blank", "noopener,noreferrer");
      } else {
        router.push(`/connectors/${h.source}`);
      }
    },
  }));

  // Pages, Settings and Actions are ordered by their best match, so "backup" leads with
  // Settings › Backups; people, topics and documents always come after them.
  const best = (g: Group) => Math.min(...g.items.map((i) => i.score ?? 3));
  const navGroups: Group[] = [
    { title: "Pages", items: navItems },
    { title: "Actions", items: actionItems },
    { title: "Settings", items: settingsItems },
  ]
    .filter((g) => g.items.length > 0)
    .sort((a, b) => best(a) - best(b));
  const groups: Group[] = [
    ...navGroups,
    { title: "People & topics", items: entityItems },
    { title: "Documents and saved items", items: recordItems },
  ].filter((g) => g.items.length > 0);
  const items = groups.flatMap((g) => g.items);

  function choose(item: Item) {
    item.go();
    onClose();
  }

  let flat = -1;
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center px-4 pt-[12vh]" style={{ background: "var(--scrim)" }} onClick={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Search"
        className="bauhaus-dialog w-full max-w-[560px] overflow-hidden flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* one input, no title or label repeating it: the dialog is named "Search" for screen readers */}
        <div className="flex items-center gap-2.5 px-4 h-14" style={{ borderBottom: "1px solid var(--border)" }}>
          <Search size={18} color="var(--ink-dim)" aria-hidden />
          <input
            autoFocus
            aria-label="Search pages, people, topics and documents"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setIndex(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setIndex((i) => Math.min(i + 1, items.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setIndex((i) => Math.max(i - 1, 0));
              } else if (e.key === "Enter" && items[index]) {
                choose(items[index]);
              }
            }}
            placeholder="Jump to a page…"
            className="flex-1 min-w-0 h-full bg-transparent text-[16px]"
            // the dialog edge is the field; no focus ring or border inside it
            style={{ color: "var(--ink)", outline: "none", boxShadow: "none", border: 0 }}
          />
          <span className="kbd shrink-0 max-sm:hidden pointer-coarse:hidden" aria-hidden>
            Esc
          </span>
          <button type="button" onClick={onClose} aria-label="Close search" className="btn btn-ghost btn-icon btn-sm sm:hidden">
            <X size={18} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
        <div className="max-h-[min(420px,60vh)] overflow-y-auto py-2">
          {groups.map((g) => (
            <div key={g.title} className="py-1">
              <div className="px-5 h-7 flex items-center text-[12px] font-medium" style={{ color: "var(--ink-dim)" }}>
                {g.title}
              </div>
              <ul>
                {g.items.map((item) => {
                  flat += 1;
                  const i = flat;
                  const Icon = item.icon;
                  const active = i === index;
                  return (
                    <li key={item.key} className="px-2">
                      <button
                        onClick={() => choose(item)}
                        onMouseMove={() => setIndex(i)}
                        className="w-full flex items-center gap-3 px-3 min-h-11 py-1.5 text-[14px] text-left rounded-[3px]"
                        style={{
                          background: active ? "var(--accent-soft)" : "transparent",
                          boxShadow: active ? "inset 3px 0 0 var(--bauhaus-app-blue)" : undefined,
                          color: "var(--ink)",
                        }}
                      >
                        <Icon size={18} color={active ? "var(--accent-text)" : "var(--ink-dim)"} />
                        <span className="flex-1 min-w-0 flex flex-col">
                          <span className="truncate">{item.label}</span>
                          {item.sub && (
                            <span className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
                              {item.sub}
                            </span>
                          )}
                        </span>
                        <ArrowUpRight size={16} color="var(--accent-text)" aria-hidden />
                      </button>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
          {items.length === 0 && (
            <div className="px-5 py-8 text-center text-[14px]" style={{ color: "var(--ink-dim)" }}>
              Nothing matches &ldquo;{query}&rdquo;. Try a shorter word, or a person&apos;s or file&apos;s name.
            </div>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 px-5 py-2.5 text-[12px]" style={{ borderTop: "1px solid var(--border)", color: "var(--ink-dim)" }}>
          {/* keyboard hints mean nothing on a touch screen: hidden on coarse pointers */}
          <span className="contents pointer-coarse:hidden">
            <span className="flex items-center gap-1.5">
              <span className="kbd">↑</span>
              <span className="kbd">↓</span> Move
            </span>
            <span className="flex items-center gap-1.5">
              <span className="kbd">↵</span> Open
            </span>
            <span className="flex items-center gap-1.5">
              <span className="kbd">Esc</span> Close
            </span>
            <span className="flex items-center gap-1.5">
              <span className="kbd">{modKey()}</span>
              <span className="kbd">K</span> Search from anywhere
            </span>
            <span className="flex items-center gap-1.5">
              <span className="kbd">?</span> All shortcuts
            </span>
          </span>
          <span className="ml-auto pointer-coarse:ml-0">Type 2 or more letters to search your memory</span>
        </div>
      </div>
    </div>
  );
}
