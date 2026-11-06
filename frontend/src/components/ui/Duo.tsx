import type { Icon as PhosphorIcon } from "@phosphor-icons/react";

export type DuoTone = "personal" | "project" | "company" | "white" | "none";

const FILL: Record<DuoTone, string> = {
  personal: "var(--vault-personal)",
  project: "var(--vault-project)",
  company: "var(--vault-company)",
  white: "#ffffff",
  none: "transparent",
};

/** A Phosphor Duotone icon with the Eunomia twist: the line stays ink (or the
 * current text colour), the back layer is a solid vault colour. Pick the tone
 * for meaning when there is one (a vault, a scope); otherwise vary the three
 * across a row so no two neighbours match. "none" drops the back layer, for
 * dense rows and buttons where a fill would be noise. Decorative by default:
 * pair it with a visible label or an aria-label on the control. */
export default function Duo({
  icon: Icon,
  size = 16,
  tone = "project",
  className = "",
  style,
  label,
}: {
  icon: PhosphorIcon;
  size?: number;
  tone?: DuoTone;
  className?: string;
  style?: React.CSSProperties;
  /** Give the icon its own accessible name when nothing else names it. */
  label?: string;
}) {
  return (
    <Icon
      size={size}
      weight="duotone"
      className={`duo shrink-0 ${className}`}
      style={{ ["--duo" as string]: FILL[tone], ...style }}
      aria-hidden={label ? undefined : true}
      aria-label={label}
      role={label ? "img" : undefined}
    />
  );
}
