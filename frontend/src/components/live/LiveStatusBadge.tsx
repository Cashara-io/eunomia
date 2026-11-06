"use client";

import { LIVE_POLL_MS } from "@/lib/queries/client";
import { useLiveStatus } from "@/lib/queries/live";

const WORDS = {
  live: { word: "Up to date", hint: "Changes made by your AI tools, in chat or in another tab show up here on their own.", color: "var(--good)" },
  connecting: { word: "Connecting…", hint: "Getting ready to show changes as they happen.", color: "var(--ink-faint)" },
  offline: {
    word: `Checking every ${Math.round(LIVE_POLL_MS / 1000)} s`,
    hint: "Live updates are paused, so this page checks for changes regularly. Use Refresh to check now.",
    color: "var(--warning)",
  },
} as const;

/** A status word (never colour alone) saying whether this page updates by itself right now. */
export default function LiveStatusBadge() {
  const status = useLiveStatus();
  const { word, hint, color } = WORDS[status];
  return (
    <span className="inline-flex items-center gap-1.5 text-[12.5px]" style={{ color: "var(--ink-dim)" }} title={hint} role="status" aria-live="polite">
      <span aria-hidden className="inline-block w-2 h-2 rounded-full shrink-0" style={{ background: color }} />
      {word}
    </span>
  );
}
