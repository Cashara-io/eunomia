"use client";

// Small shared pieces of the Memory spaces page.
import { CaretDownIcon } from "@phosphor-icons/react/ssr";
import { useId, useState, type ReactNode } from "react";
import { VaultShape, vaultTone } from "@/components/VaultChip";
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

/** Who is in a space, in one plain line: "Only you", "You and 2 others", "Private to you". */
export function peopleLine(vault: Vault, members: VaultMember[] | null): string {
  if (vault.kind === "personal") return "Private to you";
  if (!members) return "Shared space";
  const others = members.length - 1;
  if (others <= 0) return "Only you";
  return `You and ${others} ${others === 1 ? "other" : "others"}`;
}

export function RoleChip({ role }: { role: VaultRole }) {
  return (
    <span
      className="inline-flex items-center h-6 px-2 rounded-[4px] text-[12px] font-medium shrink-0"
      style={{ border: "1px solid var(--border-strong)", color: role === "owner" ? "var(--ink)" : "var(--ink-dim)" }}
    >
      {role === "owner" ? "Owner" : "Member"}
    </span>
  );
}

/* The space's identity: its vault shape in its vault colour (a red circle for the personal space, a
   blue triangle for a shared space). Decorative; the name and the words beside it carry the meaning. */
export function SpaceSwatch({ vault, size = 12 }: { vault: Vault; size?: number }) {
  return <VaultShape tone={vaultTone(vault.kind)} size={size} />;
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
          <CaretDownIcon {...ICON} aria-hidden className="shrink-0" style={{ color: "var(--ink-dim)", transform: open ? "rotate(180deg)" : undefined }} />
        </button>
      </h3>
      <div id={id} hidden={!open} className="flex flex-col gap-3">
        {open && children}
      </div>
    </section>
  );
}
