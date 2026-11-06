import type { LucideIcon } from "lucide-react";

/** The spec page header: a 30-32px identity icon (or provider mark) beside the
 * 44px title, actions on the right, a 2px ink rule, then one introduction
 * sentence that says what this page is for. */
export default function PageHeader({
  icon: Icon,
  mark,
  title,
  intro,
  actions,
  children,
}: {
  icon?: LucideIcon;
  /** A provider mark or other custom identity, used instead of `icon`. */
  mark?: React.ReactNode;
  title: React.ReactNode;
  intro?: React.ReactNode;
  actions?: React.ReactNode;
  /** Extra content under the introduction (tabs, a back link). */
  children?: React.ReactNode;
}) {
  return (
    <header className="page-header">
      <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-3">
        <div className="flex items-center gap-3 min-w-0">
          {mark ?? (Icon && <Icon size={32} strokeWidth={1.75} className="shrink-0" aria-hidden />)}
          <h1 className="page-title min-w-0 [overflow-wrap:anywhere]">{title}</h1>
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {intro && <p className="page-intro">{intro}</p>}
      {children}
    </header>
  );
}
