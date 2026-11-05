"use client";

import { useMemo, useSyncExternalStore } from "react";

// Agent-defined entity kinds ("db", "decision", ...) have no design token. Each gets a colour of its own:
// stable (hashed from the kind name, so it is the same on every screen and every visit) unless the person
// picked or shuffled one, which is remembered in this browser.

const STORAGE_KEY = "eunomia.kindColors";
const BUILT_IN = new Set(["person", "organisation", "location", "repository", "file", "symbol"]);

let overrides: Record<string, string> = {};
let loaded = false;
const listeners = new Set<() => void>();

function load() {
  if (loaded) return;
  loaded = true;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) overrides = JSON.parse(raw) as Record<string, string>;
  } catch {
    // private window or blocked storage: colours still work, they just are not remembered
  }
}

function emit() {
  for (const l of listeners) l();
}

function save() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(overrides));
  } catch {
    // not remembered; see load()
  }
}

/** Muted like the built-in kind tokens, so a new kind sits in the same palette. */
const hsl = (hue: number) => `hsl(${Math.round(hue)} 38% 62%)`;

/** FNV-1a: the same kind name always lands on the same hue. */
function hueOf(kind: string): number {
  let h = 2166136261;
  for (let i = 0; i < kind.length; i++) {
    h ^= kind.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0) % 360;
}

const isHex = (v: string) => /^#[0-9a-f]{6}$/i.test(v);

/** The colour for a kind: a built-in's own token, a picked colour, or the stable one for its name. */
export function kindColor(kind: string): string {
  if (BUILT_IN.has(kind)) return `var(--kind-${kind})`;
  load();
  return overrides[kind] ?? hsl(hueOf(kind));
}

export const isCustomKind = (kind: string) => !BUILT_IN.has(kind);

export function setKindColor(kind: string, color: string | null) {
  if (BUILT_IN.has(kind)) return;
  load();
  if (color === null) delete overrides[kind];
  else if (isHex(color)) overrides[kind] = color;
  else return;
  save();
  emit();
}

function hslToHex(h: number, sat: number, light: number): string {
  const a = sat * Math.min(light, 1 - light);
  const f = (n: number) => {
    const k = (n + h / 30) % 12;
    const c = light - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    return Math.round(c * 255).toString(16).padStart(2, "0");
  };
  return `#${f(0)}${f(8)}${f(4)}`;
}

/** The colour a kind would have without an override, as hex (what a colour input needs). */
export const defaultKindHex = (kind: string) => hslToHex(hueOf(kind), 0.38, 0.62);

/** A new random colour for the kind: a fresh hue in the same muted tone as the rest. */
export function shuffleKindColor(kind: string) {
  setKindColor(kind, hslToHex(Math.random() * 360, 0.38, 0.62));
}

/** Re-renders the caller when any kind's colour changes. Returns the lookup. */
export function useKindColor(): (kind: string) => string {
  const snapshot = useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => {
      load();
      return JSON.stringify(overrides);
    },
    () => "{}",
  );
  // a new function per change, so memoised children that take it as a prop redraw
  // eslint-disable-next-line react-hooks/exhaustive-deps
  return useMemo(() => (kind: string) => kindColor(kind), [snapshot]);
}
