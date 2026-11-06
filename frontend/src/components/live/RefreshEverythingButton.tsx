"use client";

import { ArrowsClockwiseIcon } from "@phosphor-icons/react/ssr";
import { useState } from "react";
import { useRefreshEverything } from "@/lib/queries/live";
import LiveStatusBadge from "./LiveStatusBadge";

/** "Refresh" plus the live status word: a manual check for changes made by an AI tool, chat or another tab. */
export default function RefreshEverythingButton({ label = "Refresh" }: { label?: string }) {
  const refresh = useRefreshEverything();
  const [busy, setBusy] = useState(false);
  return (
    <span className="inline-flex items-center gap-3 flex-wrap">
      <button
        type="button"
        className="btn"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            await refresh();
          } finally {
            setBusy(false);
          }
        }}
      >
        <ArrowsClockwiseIcon weight="duotone" size={16} className={busy ? "animate-spin" : undefined} aria-hidden />
        {label}
      </button>
      <LiveStatusBadge />
    </span>
  );
}
