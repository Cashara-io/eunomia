// Inspired by React Bits StatusMark (reactbits.dev). Original implementation for Eunomia.
// Usage: <SyncMark status={busy ? "running" : ok ? "done" : "idle"} size={14} /> inside a button; keep the button's text label.
// The check or cross draws only when status changes after mount, so history rows that mount "done" sit still.
"use client";

import { ArrowsClockwiseIcon } from "@phosphor-icons/react/ssr";

import { useState } from "react";
import "./bits.css";

export type SyncStatus = "idle" | "running" | "done" | "failed";

export default function SyncMark({ status, size = 14 }: { status: SyncStatus; size?: number }) {
  // Derived "changed since mount" flag (render-time state update, no effect).
  const [prev, setPrev] = useState(status);
  const [changed, setChanged] = useState(false);
  if (status !== prev) {
    setPrev(status);
    setChanged(true);
  }
  const animate = changed || undefined;

  // On a primary (blue) button both marks take the button text colour (--bits-done), so they stay readable.
  // At rest it is a plain refresh icon, so it never reads as a frozen spinner
  // (and under reduced motion "running" still differs from idle).
  if (status === "idle") return <ArrowsClockwiseIcon weight="duotone" size={size} aria-hidden style={{ flexShrink: 0 }} />;

  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden style={{ flexShrink: 0 }}>
      {status === "running" && (
        // 68% of the circle, rotating while running.
        // The spin lives on the wrapper: a CSS transform on the circle itself
        // would replace its rotate() attribute and swing it off-centre.
        <g className="bits-spin">
          <circle cx="8" cy="8" r="5.5" stroke="currentColor" pathLength={100} strokeDasharray="68 100" transform="rotate(-90 8 8)" />
        </g>
      )}
      {status === "done" && <path d="M3.5 8.4 6.6 11.3 12.5 4.9" stroke="var(--bits-done, var(--good))" pathLength={1} className="bits-draw" data-animate={animate} />}
      {status === "failed" && (
        <path d="M4.5 4.5 11.5 11.5M11.5 4.5 4.5 11.5" stroke="var(--bits-done, var(--critical))" pathLength={1} className="bits-draw" data-animate={animate} />
      )}
    </svg>
  );
}
