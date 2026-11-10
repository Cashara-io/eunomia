
/** The page header: a shared 22px title (a provider mark beside it when there is
 * one), actions on the right, a hairline rule, then at most one short line
 * that says what the page is for. `icon` is accepted but not drawn: the nav
 * already carries the page's icon. */
export default function PageHeader({
  mark,
  title,
  intro,
  actions,
  children,
}: {
  /** Accepted for callers that still pass one (a Phosphor icon); not drawn. */
  icon?: React.ElementType;
  /** Accepted from main's design-system callers; this header has a single tone. */
  tone?: string;
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
          {mark}
          <h1 className="page-title min-w-0 [overflow-wrap:anywhere]">{title}</h1>
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {/* a div, not a p: callers pass breadcrumbs (<nav>) and other blocks as the intro */}
      {intro && <div className="page-intro">{intro}</div>}
      {children}
    </header>
  );
}
