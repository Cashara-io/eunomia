// Original implementation for Cupel: shared motion helpers for the bits components.
// Usage: const reduced = useReducedMotion(); const fine = useFinePointer(); cssVar("--accent") reads a token at fire time.
// Mount <InputModeTracker /> once per page tree; it sets data-input="pointer"|"keyboard" on <html> for CSS to gate motion.
"use client";

import { useEffect, useSyncExternalStore } from "react";

function mediaStore(query: string) {
  return {
    subscribe(cb: () => void) {
      const m = window.matchMedia(query);
      m.addEventListener("change", cb);
      return () => m.removeEventListener("change", cb);
    },
    get: () => window.matchMedia(query).matches,
  };
}

const reducedStore = mediaStore("(prefers-reduced-motion: reduce)");
const fineStore = mediaStore("(hover: hover) and (pointer: fine)");

/** True when the user asked for less motion. Server snapshot is true, so SSR never animates. */
export function useReducedMotion(): boolean {
  return useSyncExternalStore(reducedStore.subscribe, reducedStore.get, () => true);
}

/** True for a mouse or trackpad that can hover. Server snapshot is false. */
export function useFinePointer(): boolean {
  return useSyncExternalStore(fineStore.subscribe, fineStore.get, () => false);
}

/** Imperative check for event handlers and effects (never call during render). */
export function prefersReducedMotion(): boolean {
  return reducedStore.get();
}

/** Resolved value of a CSS custom property on <html>, read now (so theme swaps are honoured). */
export function cssVar(name: string): string {
  const prop = name.startsWith("--") ? name : `--${name}`;
  return getComputedStyle(document.documentElement).getPropertyValue(prop).trim();
}

// ---- input mode ----

export type InputMode = "pointer" | "keyboard";

let mode: InputMode | undefined;
const listeners = new Set<() => void>();
let trackers = 0;

function setMode(next: InputMode) {
  if (next === mode) return;
  mode = next;
  document.documentElement.dataset.input = next;
  listeners.forEach((l) => l());
}
const onPointer = () => setMode("pointer");
const onKey = (e: KeyboardEvent) => {
  // A lone modifier (Shift for shift-click, Cmd for cmd-click) is still pointer work.
  if (e.key === "Shift" || e.key === "Meta" || e.key === "Control" || e.key === "Alt") return;
  setMode("keyboard");
};

function subscribeMode(cb: () => void) {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/** Last input the user reached for, undefined until they touch anything. */
export function useInputMode(): InputMode | undefined {
  return useSyncExternalStore(subscribeMode, () => mode, () => undefined);
}

/** Renders nothing. Safe to mount more than once; listeners are shared. */
export function InputModeTracker(): null {
  useEffect(() => {
    if (trackers++ === 0) {
      window.addEventListener("pointerdown", onPointer, true);
      window.addEventListener("keydown", onKey, true);
    }
    return () => {
      if (--trackers === 0) {
        window.removeEventListener("pointerdown", onPointer, true);
        window.removeEventListener("keydown", onKey, true);
      }
    };
  }, []);
  return null;
}
