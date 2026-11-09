"use client";

import { setHapticsEnabled, useHapticsEnabled } from "@/lib/haptics";
import { PanelHead } from "./shared";

export function AccessibilitySection() {
  const on = useHapticsEnabled();
  return (
    <section className="flex flex-col gap-5">
      <PanelHead title="Accessibility">Preferences for this browser.</PanelHead>
      <div className="ledger p-5 flex items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <label htmlFor="haptic-feedback" className="text-[14px] font-medium">
            Haptic feedback
          </label>
          <p
            id="haptic-help"
            className="text-[13px]"
            style={{ color: "var(--ink-dim)" }}
          >
            Gentle feedback when you send, save, copy or review. Works on
            supported phones. Off when your device asks for reduced motion.
          </p>
        </div>
        <input
          id="haptic-feedback"
          type="checkbox"
          role="switch"
          aria-describedby="haptic-help"
          checked={on}
          onChange={(e) => setHapticsEnabled(e.target.checked)}
          className="mt-1 h-5 w-5 shrink-0 accent-[var(--accent)]"
        />
      </div>
    </section>
  );
}
