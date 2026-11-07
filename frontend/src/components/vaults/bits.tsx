"use client";

// Small shared pieces of the Memory spaces page.
import Link from "next/link";
import { ArrowRightIcon, CaretDownIcon, XIcon } from "@phosphor-icons/react/ssr";
import { useId, useState, type ReactNode } from "react";
import type { Vault, VaultMember, VaultRole } from "@/lib/types";

export const ICON = { size: 18, weight: "duotone" } as const;

export function ago(iso: string | null): string | null {
  if (!iso) return null;
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

export const sameEmail = (a: string | null, b: string | null) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** Who is in a space, in one plain line: "Not shared yet", "3 members", "Private to you". */
export function peopleLine(vault: Vault, members: VaultMember[] | null): string {
  if (vault.kind === "personal") return "Private to you";
  if (!members) return "Team space";
  if (members.length <= 1) return "Not shared yet";
  return `${members.length} members`;
}

/** A member's role in plain dim text, for anyone who can't change it. */
export function RoleChip({ role }: { role: VaultRole }) {
  return (
    <span className="text-[13px] shrink-0" style={{ color: "var(--ink-dim)" }}>
      {role === "owner" ? "Owner" : "Member"}
    </span>
  );
}

/* A space's series colour on the Memory map, where several spaces can show at once.
   Muted hues, never the vault colours: those name the kind of space, not which one. */
const SWATCHES = ["var(--space-1)", "var(--space-2)", "var(--space-3)"];
/** A space's colour on the Memory map. */
export function spaceColor(vault: Pick<Vault, "id" | "kind"> | undefined): string {
  if (!vault || vault.kind === "personal") return "var(--ink-faint)";
  let h = 0;
  for (const ch of vault.id) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return SWATCHES[h % SWATCHES.length];
}
/* A check that draws itself once on mount (the shared .bits-draw grammar; reduced motion is handled there). */
export function DrawnCheck() {
  return (
    <svg width={14} height={14} viewBox="0 0 16 16" fill="none" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" aria-hidden style={{ flexShrink: 0 }}>
      <path d="M3.5 8.4 6.6 11.3 12.5 4.9" stroke="var(--good)" pathLength={1} className="bits-draw" data-animate="" />
    </svg>
  );
}

/** One section of the space detail: a heading and its content, separated by a hairline. */
export function DetailSection({ title, aside, children }: { title: ReactNode; aside?: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <section className="px-5 sm:px-6 py-5 flex flex-col gap-3" style={{ borderTop: "1px solid var(--border)" }} aria-labelledby={id}>
      <div className="flex items-center justify-between gap-2">
        <h3 id={id} className="section-title flex items-center gap-2">
          {title}
        </h3>
        {aside}
      </div>
      {children}
    </section>
  );
}

/** A section that stays folded behind its heading until someone opens it. */
export function Disclosure({
  title,
  hint,
  defaultOpen = false,
  children,
}: {
  title: string;
  hint?: string;
  defaultOpen?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const id = useId();
  return (
    <section className="px-5 sm:px-6 py-4 flex flex-col gap-3" style={{ borderTop: "1px solid var(--border)" }}>
      <h3>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen((o) => !o)}
          className="w-full flex items-center gap-2 min-h-10 text-left rounded-[10px]"
        >
          <span className="flex-1 min-w-0 flex flex-col">
            <span className="section-title">{title}</span>
            {hint && !open && (
              <span className="text-[13px] font-normal" style={{ color: "var(--ink-dim)" }}>
                {hint}
              </span>
            )}
          </span>
          <CaretDownIcon {...ICON} aria-hidden className="shrink-0" style={{ color: "var(--ink-dim)", transform: open ? "rotate(180deg)" : undefined }} />
        </button>
      </h3>
      <div id={id} hidden={!open} className="flex flex-col gap-3">
        {open && children}
      </div>
    </section>
  );
}

/** Who sent an invitation, in words: their email when known, else "Someone". */
export const inviterWords = (invitedBy: string | null | undefined) => invitedBy || "Someone";

/** After joining a space: says so, with the way in to what it holds. */
export function JoinedNotice({ id, name, onDismiss }: { id: string; name: string; onDismiss?: () => void }) {
  return (
    <div role="status" className="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-2.5 text-[14px]">
      <DrawnCheck />
      <span className="flex-1 min-w-[160px]">
        You joined <span className="font-medium">{name}</span>.
      </span>
      <Link href={`/entities?vault=${encodeURIComponent(id)}`} className="btn btn-sm">
        See its people &amp; topics
        <ArrowRightIcon {...ICON} aria-hidden />
      </Link>
      {onDismiss && (
        <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label="Dismiss" onClick={onDismiss}>
          <XIcon {...ICON} aria-hidden />
        </button>
      )}
    </div>
  );
}
