import type { Icon as PhosphorIcon } from "@phosphor-icons/react";
import Duo, { type DuoTone } from "./Duo";

/** The page header: a 24px Phosphor Duotone identity icon (or provider mark)
 * beside the page title, actions on the right, a hairline, then one
 * introduction sentence that says what this page is for. */
export default function PageHeader({
  icon,
  tone = "project",
  mark,
  title,
  intro,
  actions,
  children,
}: {
  icon?: PhosphorIcon;
  /** The icon's back-layer colour. Vary it between pages; never a status. */
  tone?: DuoTone;
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
          {mark ?? (icon && <Duo icon={icon} size={26} tone={tone} />)}
          <h1 className="page-title min-w-0 [overflow-wrap:anywhere]">{title}</h1>
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {intro && <div className="page-intro">{intro}</div>}
      {children}
    </header>
  );
}
