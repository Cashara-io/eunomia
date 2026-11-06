"use client";

import { useSyncExternalStore } from "react";
import { Moon, Sun } from "@phosphor-icons/react/ssr";

type Theme = "light" | "dark";
const KEY = "eunomia-theme";
const listeners = new Set<() => void>();

function read(): Theme {
  return document.documentElement.dataset.theme === "dark" ? "dark" : "light";
}

function write(next: Theme) {
  if (next === "dark") document.documentElement.dataset.theme = "dark";
  else delete document.documentElement.dataset.theme;
  try {
    if (next === "dark") localStorage.setItem(KEY, "dark");
    else localStorage.removeItem(KEY);
  } catch {
    // Private windows can refuse storage; the switch still works for this visit.
  }
  listeners.forEach((l) => l());
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/** Light is the default; dark ("ink paper") is opt-in and saved per browser. */
export function useTheme(): [Theme, (t: Theme) => void] {
  const theme = useSyncExternalStore(subscribe, read, () => "light" as Theme);
  return [theme, write];
}

export default function ThemeToggle({ className = "" }: { className?: string }) {
  const [theme, setTheme] = useTheme();
  const dark = theme === "dark";
  return (
    <button
      type="button"
      onClick={() => setTheme(dark ? "light" : "dark")}
      aria-pressed={dark}
      aria-label="Dark theme"
      className={`btn btn-ghost btn-sm btn-icon ${className}`}
    >
      {dark ? <Moon size={16} weight="duotone" className="duo" aria-hidden /> : <Sun size={16} weight="duotone" className="duo" aria-hidden />}
    </button>
  );
}
