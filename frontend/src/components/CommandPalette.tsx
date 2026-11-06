"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { ArrowUpRight, Building2, MapPin, FileText, FolderGit2, Plug, Search, User, X } from "lucide-react";
import { ADVANCED_NAV, MAIN_NAV } from "./Sidebar";
import { documentHref, type EntityKind } from "@/lib/types";
import { useSources } from "@/lib/queries/sources";
import { usePaletteSearch } from "@/lib/queries/tools";

type IconType = React.ComponentType<{ size?: number; color?: string; strokeWidth?: number }>;
type Item = { key: string; label: string; sub?: string; icon: IconType; go: () => void };
type Group = { title: string; items: Item[] };

const OPEN_EVENT = "eunomia:open-palette";
/** Opens the palette from anywhere (the sidebar search button, page CTAs). */
export function openCommandPalette() {
  window.dispatchEvent(new Event(OPEN_EVENT));
}

// organisation and location read as a building and a pin, not a person
const ENTITY_ICON: Record<string, IconType> = { person: User, organisation: Building2, location: MapPin, repository: FolderGit2, file: FileText };

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
  return open ? <Palette onClose={() => setOpen(false)} /> : null;
}

function Palette({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const rows = useSources().data ?? [];
  const [debounced, setDebounced] = useState("");
  const router = useRouter();

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
  const staticItems: Item[] = staticNav().map((s) => ({
    key: `nav:${s.href}`,
    label: s.label,
    icon: s.icon,
    go: () => router.push(s.href),
  }));
  const navItems = [...staticItems, ...connectorItems].filter((item) =>
    item.label.toLowerCase().includes(query.toLowerCase())
  );

  const entityItems: Item[] = (searching ? entityHits : []).map((e) => ({
    key: `entity:${e.id}`,
    label: e.name,
    sub: KIND_WORD[e.kind] ?? e.kind,
    icon: ENTITY_ICON[e.kind] ?? User,
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

  const groups: Group[] = [
    { title: "Pages", items: navItems },
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
        aria-labelledby="palette-title"
        className="bauhaus-dialog w-full max-w-[560px] overflow-hidden flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between gap-2 px-5 pt-4">
          <h2 id="palette-title" className="text-[20px] font-semibold tracking-[-0.01em]">
            Search
          </h2>
          <button type="button" onClick={onClose} aria-label="Close search" className="btn btn-ghost btn-icon btn-sm">
            <X size={18} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
        <div className="px-5 pt-2 pb-3 flex flex-col gap-1.5" style={{ borderBottom: "1px solid var(--border)" }}>
          <label htmlFor="palette-input" className="text-[13px] font-medium">
            Search pages, people, topics and documents
          </label>
          <div className="field flex items-center gap-2.5 px-3 h-11">
            <Search size={18} color="var(--ink-dim)" aria-hidden />
            <input
              id="palette-input"
              autoFocus
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
              className="flex-1 h-full bg-transparent text-[15px] outline-none"
              style={{ color: "var(--ink)" }}
            />
          </div>
        </div>
        <div className="max-h-[min(420px,60vh)] overflow-y-auto py-2">
          {groups.map((g) => (
            <div key={g.title} className="py-1">
              <div className="caps px-5 h-7 flex items-center" style={{ color: "var(--ink-dim)" }}>
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
          <span className="ml-auto">Type 2 or more letters to search your memory</span>
        </div>
      </div>
    </div>
  );
}
