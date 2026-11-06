"use client";

// Small shared pieces of the Memory spaces page.
import Link from "next/link";
import { ArrowRight, ChevronDown, X } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import type { Vault, VaultMember, VaultRole } from "@/lib/types";

export const ICON = { size: 18, strokeWidth: 1.75 } as const;

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
  if (!members) return "Shared space";
  if (members.length <= 1) return "Not shared yet";
  return `${members.length} members`;
}

/** A small neutral circle with someone's initials, for member rows. Decorative: the email beside it is the name. */
export function Initials({ email }: { email: string }) {
  const local = email.split("@")[0] ?? "";
  const parts = local.split(/[._-]+/).filter(Boolean);
  const letters = (parts.length > 1 ? parts[0][0] + parts[1][0] : local.slice(0, 2)).toUpperCase();
  return (
    <span
      aria-hidden
      className="inline-grid place-items-center shrink-0 w-7 h-7 rounded-full text-[12px] font-medium"
      style={{ background: "var(--surface-raised)", border: "1px solid var(--border)", color: "var(--ink-dim)" }}
    >
      {letters || "?"}
    </span>
  );
}

export function RoleChip({ role }: { role: VaultRole }) {
  return (
    <span
      className="inline-flex items-center h-6 px-2 rounded-[3px] text-[12px] font-medium shrink-0"
      style={{ border: "1px solid var(--border-strong)", color: role === "owner" ? "var(--ink)" : "var(--ink-dim)" }}
    >
      {role === "owner" ? "Owner" : "Member"}
    </span>
  );
}

/* The space's identity: a small dot, neutral grey for the personal space and one
   of a few calm colours for shared spaces (never the danger red). Decorative; the
   name and the words beside it carry the meaning. */
const SWATCHES = ["var(--bauhaus-app-yellow)", "var(--bauhaus-app-blue)", "var(--good)"];
export function SpaceSwatch({ vault, size = 8 }: { vault: Vault; size?: number }) {
  const personal = vault.kind === "personal";
  let h = 0;
  for (const ch of vault.id) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return (
    <span
      aria-hidden
      className="inline-block shrink-0"
      style={{
        width: size,
        height: size,
        background: personal ? "var(--ink-faint)" : SWATCHES[h % SWATCHES.length],
        borderRadius: 999,
      }}
    />
  );
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
export function DetailSection({ title, icon, aside, children }: { title: ReactNode; icon?: ReactNode; aside?: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <section className="px-5 sm:px-6 py-5 flex flex-col gap-3" style={{ borderTop: "1px solid var(--border)" }} aria-labelledby={id}>
      <div className="flex items-center justify-between gap-2">
        <h3 id={id} className="section-title flex items-center gap-2">
          {icon}
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
  icon,
  hint,
  defaultOpen = false,
  children,
}: {
  title: string;
  icon?: ReactNode;
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
          className="w-full flex items-center gap-2 min-h-10 text-left rounded-[4px]"
        >
          {icon}
          <span className="flex-1 min-w-0 flex flex-col">
            <span className="section-title">{title}</span>
            {hint && !open && (
              <span className="text-[13px] font-normal" style={{ color: "var(--ink-dim)" }}>
                {hint}
              </span>
            )}
          </span>
          <ChevronDown {...ICON} aria-hidden className="shrink-0" style={{ color: "var(--ink-dim)", transform: open ? "rotate(180deg)" : undefined }} />
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
    <div role="status" className="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-2.5 text-[14px]" style={{ borderTop: "1px solid var(--border)" }}>
      <DrawnCheck />
      <span className="flex-1 min-w-[160px]">
        You joined <span className="font-medium">{name}</span>.
      </span>
      <Link href={`/entities?vault=${encodeURIComponent(id)}`} className="btn btn-sm">
        See its people &amp; topics
        <ArrowRight {...ICON} aria-hidden />
      </Link>
      {onDismiss && (
        <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label="Dismiss" onClick={onDismiss}>
          <X {...ICON} aria-hidden />
        </button>
      )}
    </div>
  );
}
