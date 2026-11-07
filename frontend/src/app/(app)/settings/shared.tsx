"use client";

import { useState } from "react";
import { XIcon } from "@phosphor-icons/react/ssr";
import CopyButton from "@/components/bits/CopyButton";
import DecryptReveal from "@/components/bits/DecryptReveal";

export const ICON = { size: 14, weight: "duotone" } as const;

/** A destructive row action that stays quiet at rest and turns red on hover or focus (the Spaces
 * pattern); its confirm step says the rest. `!` because the .btn styles sit outside Tailwind's layers. */
export const QUIET_DANGER = "btn btn-ghost btn-sm text-[var(--ink-dim)]! hover:text-[var(--critical)]! focus-visible:text-[var(--critical)]!";

export type Tone = "good" | "warning" | "critical" | "accent" | "off";
const TONE: Record<Tone, string> = {
  good: "var(--good)",
  warning: "var(--warning)",
  critical: "var(--critical)",
  accent: "var(--accent)",
  off: "var(--ink-faint)",
};

/** A section's one-line status: a 6px dot and a few words ("Off", "On · expires 3 Feb"). */
export function StatusLine({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return (
    <p className="flex items-center gap-2 text-[13px] min-w-0" style={{ color: "var(--ink)" }}>
      <span className="dot" style={{ background: TONE[tone] }} aria-hidden />
      <span role="status" className="min-w-0">
        {children}
      </span>
    </p>
  );
}

/* reveal: a freshly minted secret decrypts in once (the token, never a command). */
export function CopyField({ value, reveal = false }: { value: string; reveal?: boolean }) {
  return (
    <div className="field flex items-center gap-2 min-h-10 pl-3 pr-1">
      <code className="flex-1 min-w-0 truncate text-[13px] font-mono" style={{ color: "var(--ink)" }}>
        {reveal ? <DecryptReveal key={value} text={value} /> : value}
      </code>
      <CopyButton value={value} size="sm" className="shrink-0" />
    </div>
  );
}

export function relativeTime(iso: string | null): string {
  if (!iso) return "never";
  const ms = Date.now() - new Date(iso).getTime();
  const mins = Math.round(ms / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  return `${days}d ago`;
}

/** "in 12d" / "Expired": when a token stops working. */
export function expiryLabel(iso: string | null): { text: string; expired: boolean } {
  if (!iso) return { text: "Never", expired: false };
  const ms = new Date(iso).getTime() - Date.now();
  if (ms <= 0) return { text: "Expired", expired: true };
  const days = Math.ceil(ms / 86_400_000);
  return { text: days >= 2 ? `in ${days}d` : "in <1d", expired: false };
}

/* Section header inside a tab panel: title, an optional one-line status, one line of purpose, optional action. */
export function PanelHead({
  title,
  children,
  action,
  status,
}: {
  title: string;
  children?: React.ReactNode;
  action?: React.ReactNode;
  status?: React.ReactNode;
}) {
  return (
    // phones: the action (tabs, a button) goes under the heading instead of squeezing it
    <div className="flex flex-col items-start gap-3 sm:flex-row sm:flex-wrap sm:justify-between">
      <div className="min-w-0 flex-1 self-stretch sm:self-auto">
        {/* the status sits on the heading line ("HTTPS · Off"), never floating on its own row */}
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h2 className="section-title">{title}</h2>
          {status}
        </div>
        {children && (
          <p className="text-[15px] leading-relaxed mt-1.5 max-w-[68ch]" style={{ color: "var(--ink-dim)" }}>
            {children}
          </p>
        )}
      </div>
      {action}
    </div>
  );
}

/* Inline revoke: quiet text that turns red on hover; one click arms it, the second confirms. */
export function RevokeButton({ label, onRevoke, warn }: { label: string; onRevoke: () => Promise<void>; warn?: string }) {
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  async function confirm() {
    setBusy(true);
    try {
      await onRevoke();
    } finally {
      setBusy(false);
      setArmed(false);
    }
  }
  if (!armed)
    return (
      <button type="button" onClick={() => setArmed(true)} disabled={busy} aria-label="Revoke" title={`Revoke ${label}`} className={QUIET_DANGER}>
        Revoke
      </button>
    );
  return (
    <span className="inline-flex items-center gap-1">
      {warn && <span className="text-[13px] mr-1" style={{ color: "var(--critical)" }}>{warn}</span>}
      <button type="button" onClick={confirm} disabled={busy} className="btn btn-danger-solid btn-sm">
        {busy ? "Revoking…" : "Revoke"}
      </button>
      <button type="button" onClick={() => setArmed(false)} aria-label="Keep" className="btn btn-ghost btn-sm btn-icon">
        <XIcon {...ICON} />
      </button>
    </span>
  );
}

export function TableSkeleton({ cols }: { cols: number }) {
  return (
    <>
      {[0, 1].map((i) => (
        <tr key={i} aria-hidden>
          {Array.from({ length: cols }, (_, c) => (
            <td key={c}>
              <span className="skeleton block h-4" style={{ width: c === 0 ? "60%" : "50%" }} />
            </td>
          ))}
        </tr>
      ))}
    </>
  );
}
