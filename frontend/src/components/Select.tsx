"use client";

// One select for the whole app, in the canvas style. ARIA "select-only combobox":
// focus stays on the trigger, the list is announced through aria-activedescendant.
// Usage: <Select aria-label="Role" value={role} onChange={setRole} options={[{ value: "member", label: "Member" }]} />

import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";

export type SelectOption = { value: string; label: string; hint?: string };

type Props = {
  value: string;
  onChange: (value: string) => void;
  options: SelectOption[];
  id?: string;
  "aria-label"?: string;
  placeholder?: string;
  disabled?: boolean;
  mono?: boolean;
  /** Size and width classes for the trigger, e.g. "h-8 text-[13px] w-full". */
  className?: string;
};

const GAP = 4;
const EDGE = 8;
const MAX_H = 288;
const TYPEAHEAD_MS = 500;

type Place = { left: number; top: number; width: number; maxH: number; up: boolean };

export default function Select({ value, onChange, options, id, placeholder = "Select…", disabled, mono, className = "h-10 text-[14px]", ...rest }: Props) {
  const listId = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [open, setOpen] = useState(false);
  // Pointer opens get the pop; keyboard opens appear at once (opened many times, never animate).
  const [animate, setAnimate] = useState(false);
  const [active, setActive] = useState(0);
  const [place, setPlace] = useState<Place | null>(null);
  const typed = useRef({ text: "", at: 0 });

  const selectedIndex = options.findIndex((o) => o.value === value);
  const selected = options[selectedIndex];

  function show(byPointer: boolean) {
    if (disabled || options.length === 0) return;
    setActive(Math.max(0, selectedIndex));
    setAnimate(byPointer);
    setOpen(true);
  }
  function close(refocus = true) {
    setOpen(false);
    setPlace(null);
    if (refocus) trigger.current?.focus();
  }
  function choose(i: number) {
    const o = options[i];
    if (o && o.value !== value) onChange(o.value);
    close();
  }

  // Below the trigger, or above when there is not enough room; never wider than the viewport.
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const r = trigger.current?.getBoundingClientRect();
      if (!r) return;
      const below = window.innerHeight - r.bottom - GAP - EDGE;
      const above = r.top - GAP - EDGE;
      const up = below < Math.min(MAX_H, 160) && above > below;
      const width = Math.min(Math.max(r.width, 160), window.innerWidth - EDGE * 2);
      const left = Math.min(Math.max(EDGE, r.left), window.innerWidth - EDGE - width);
      setPlace({ left, top: up ? r.top - GAP : r.bottom + GAP, width, maxH: Math.min(MAX_H, up ? above : below), up });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open]);

  // Click outside closes without stealing focus back.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!trigger.current?.contains(t) && !list.current?.contains(t)) close(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);

  // Keep the active option in view while moving with the keyboard.
  useEffect(() => {
    if (open) list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [open, active, place]);

  function typeahead(key: string) {
    const now = performance.now();
    const t = typed.current;
    t.text = now - t.at > TYPEAHEAD_MS ? key : t.text + key;
    t.at = now;
    const start = open ? active : Math.max(0, selectedIndex);
    const order = [...options.keys()].map((k) => (start + 1 + k) % options.length);
    // A repeated single letter cycles; a longer prefix searches from the current item.
    const repeat = t.text.length > 1 && [...t.text].every((c) => c === t.text[0]);
    const needle = (repeat ? t.text[0] : t.text).toLowerCase();
    const from = repeat || t.text.length === 1 ? order : [start, ...order];
    return from.find((i) => options[i].label.toLowerCase().startsWith(needle));
  }

  function onKeyDown(e: React.KeyboardEvent) {
    const last = options.length - 1;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
        e.preventDefault();
        show(false);
      } else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const i = typeahead(e.key);
        if (i !== undefined && options[i].value !== value) onChange(options[i].value);
      }
      return;
    }
    const move = (i: number) => {
      e.preventDefault();
      setActive(Math.min(last, Math.max(0, i)));
    };
    if (e.key === "ArrowDown") move(active + 1);
    else if (e.key === "ArrowUp") move(active - 1);
    else if (e.key === "Home") move(0);
    else if (e.key === "End") move(last);
    else if (e.key === "PageDown") move(active + 8);
    else if (e.key === "PageUp") move(active - 8);
    else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      choose(active);
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Tab") {
      choose(active);
    } else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) {
      const i = typeahead(e.key);
      if (i !== undefined) setActive(i);
    }
  }

  return (
    <>
      <button
        ref={trigger}
        id={id}
        type="button"
        role="combobox"
        aria-label={rest["aria-label"]}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? `${listId}-${active}` : undefined}
        disabled={disabled}
        onClick={(e) => (open ? close() : show(e.detail > 0))}
        onKeyDown={onKeyDown}
        className={`field select-trigger inline-flex items-center gap-2 pl-3 pr-2.5 text-left disabled:opacity-50 ${className}`}
        data-open={open || undefined}
      >
        <span className={`flex-1 min-w-0 truncate ${mono ? "font-mono" : ""}`} style={{ color: selected ? "var(--ink)" : "var(--ink-faint)" }}>
          {selected ? selected.label : placeholder}
        </span>
        <ChevronDown size={16} strokeWidth={1.75} aria-hidden className="select-chevron shrink-0" />
      </button>
      {open &&
        place &&
        createPortal(
          <ul
            ref={list}
            id={listId}
            role="listbox"
            aria-label={rest["aria-label"]}
            className={`select-list panel ${animate ? "pop-in" : ""}`}
            style={{
              left: place.left,
              width: place.width,
              maxHeight: place.maxH,
              ...(place.up ? { bottom: window.innerHeight - place.top } : { top: place.top }),
              transformOrigin: place.up ? "bottom center" : "top center",
            }}
          >
            {options.map((o, i) => {
              const isSelected = o.value === value;
              return (
                <li
                  key={o.value}
                  id={`${listId}-${i}`}
                  data-index={i}
                  role="option"
                  aria-selected={isSelected}
                  data-active={i === active || undefined}
                  onPointerMove={() => setActive(i)}
                  // Keep focus on the trigger: the list never takes it.
                  onPointerDown={(e) => e.preventDefault()}
                  onClick={() => choose(i)}
                  className="select-option"
                >
                  <span className="w-3.5 shrink-0" aria-hidden>
                    {isSelected && <Check size={14} strokeWidth={2} />}
                  </span>
                  <span className={`flex-1 min-w-0 truncate ${mono ? "font-mono" : ""}`}>{o.label}</span>
                  {o.hint && <span className="shrink-0 text-[12px]" style={{ color: "var(--ink-dim)" }}>{o.hint}</span>}
                </li>
              );
            })}
          </ul>,
          document.body,
        )}
    </>
  );
}
