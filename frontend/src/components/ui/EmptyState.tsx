import type { ReactNode } from "react";

/** The one empty state: a left-aligned sentence (bold lead-in optional) and at most one action.
 * No icon, no card, no centring: it sits where the content would be. */
export default function EmptyState({ title, children, action }: { title?: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-3 py-6 max-w-[60ch]">
      <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
        {title && <strong className="font-medium" style={{ color: "var(--ink)" }}>{title} </strong>}
        {children}
      </p>
      {action}
    </div>
  );
}
