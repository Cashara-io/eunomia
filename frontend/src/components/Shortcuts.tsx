"use client";

// App-wide keyboard shortcuts, mounted once from CommandPalette. Ignored while typing in a field.
//   g then h / c / p / s / d / ,  go to a page
//   /                             focus the page's search, else open the palette
//   ?                             this list

import { useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { X } from "lucide-react";
import { modKey } from "@/lib/platform";

const GO: { key: string; href: string; label: string }[] = [
  { key: "h", href: "/", label: "Home" },
  { key: "c", href: "/chat", label: "Chat" },
  { key: "p", href: "/entities", label: "People & topics" },
  { key: "s", href: "/vaults", label: "Memory spaces" },
  { key: "d", href: "/documents", label: "Documents" },
  { key: ",", href: "/settings", label: "Settings" },
];
const CHORD_MS = 1500; // time allowed between "g" and the page key

function typing(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

const visible = (el: HTMLElement) => el.offsetParent !== null || el.getClientRects().length > 0;

// The page's own search or find box: one marked `data-page-search`, else a search-type input, else
// the first field whose name says it searches, finds or filters.
function pageSearch(): HTMLInputElement | null {
  const marked = [...document.querySelectorAll<HTMLInputElement>("main [data-page-search], main input[type='search']")].find(visible);
  if (marked) return marked;
  return (
    [...document.querySelectorAll<HTMLInputElement>("main input")].find(
      (el) => visible(el) && /^(search|find|filter)/i.test(el.getAttribute("aria-label") || el.placeholder || ""),
    ) ?? null
  );
}

export default function Shortcuts({ paletteOpen, openPalette }: { paletteOpen: boolean; openPalette: () => void }) {
  const router = useRouter();
  const [sheet, setSheet] = useState(false);
  const chord = useRef(0); // when "g" was pressed, 0 when no chord is pending

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
      if (e.key === "Escape") {
        setSheet(false);
        return;
      }
      if (paletteOpen || typing(e.target)) return;
      if (document.querySelector("[aria-modal='true']:not([data-shortcuts-sheet])")) return; // another dialog has the keyboard

      if (chord.current && Date.now() - chord.current < CHORD_MS) {
        chord.current = 0;
        const hit = GO.find((g) => g.key === e.key.toLowerCase());
        if (hit) {
          e.preventDefault();
          setSheet(false);
          router.push(hit.href);
        }
        return;
      }
      chord.current = 0;
      if (e.key === "g" && !e.shiftKey) {
        chord.current = Date.now();
      } else if (e.key === "/") {
        e.preventDefault();
        setSheet(false);
        const field = pageSearch();
        if (field) {
          field.focus();
          field.select();
        } else openPalette();
      } else if (e.key === "?") {
        e.preventDefault();
        setSheet((v) => !v);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen, openPalette, router]);

  if (!sheet) return null;
  const rows: { keys: string[]; label: string }[] = [
    ...GO.map((g) => ({ keys: ["g", g.key], label: g.label })),
    { keys: ["/"], label: "Search this page, or open search" },
    { keys: [modKey(), "K"], label: "Search everything" },
    { keys: ["?"], label: "Show this list" },
  ];
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center px-4 pt-[12vh]" style={{ background: "var(--scrim)" }} onClick={() => setSheet(false)}>
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-title"
        data-shortcuts-sheet
        className="bauhaus-dialog w-full max-w-[400px] p-5 flex flex-col gap-3"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between gap-2">
          <h2 id="shortcuts-title" className="text-[16px] font-semibold">
            Keyboard shortcuts
          </h2>
          <button type="button" autoFocus className="btn btn-ghost btn-icon btn-sm" aria-label="Close" onClick={() => setSheet(false)}>
            <X size={18} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
        <ul className="hairline-rows text-[14px]">
          {rows.map((r) => (
            <li key={r.label} className="flex items-center justify-between gap-4 py-2">
              <span>{r.label}</span>
              <span className="flex items-center gap-1 shrink-0">
                {r.keys.map((k, i) => (
                  <span key={i} className="kbd">
                    {k}
                  </span>
                ))}
              </span>
            </li>
          ))}
        </ul>
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          Press g, then the letter. Shortcuts are off while you type in a field.
        </p>
      </div>
    </div>
  );
}
