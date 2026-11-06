import type { VaultKind } from "@/lib/types";

/** The three vault identities, each a flat colour paired with a shape.
 * "mixed" is any vault that is none of them: an ink outline square, never a
 * fourth colour. */
export type VaultTone = "personal" | "project" | "company" | "mixed";

/** Today the server has personal and shared ("org") vaults. A shared space is
 * everyone's in that team, so it takes the company identity. */
export function vaultTone(kind: VaultKind | string | null | undefined): VaultTone {
  if (kind === "personal") return "personal";
  if (kind === "org" || kind === "company") return "company";
  if (kind === "project") return "project";
  return "mixed";
}

const WORD: Record<VaultTone, string> = {
  personal: "Personal space",
  project: "Project space",
  company: "Shared space",
  mixed: "Space",
};

/** A vault's shape on its own: a 12px mark before a name in a list row. */
export function VaultShape({ tone, size = 12, fill, className = "" }: { tone: VaultTone; size?: number; fill?: string; className?: string }) {
  const color = fill ?? (tone === "mixed" ? "var(--ink)" : `var(--vault-${tone})`);
  return (
    <svg width={size} height={size} viewBox="0 0 12 12" aria-hidden className={`shrink-0 ${className}`}>
      {tone === "personal" && <circle cx="6" cy="6" r="6" fill={color} />}
      {tone === "project" && <rect width="12" height="12" fill={color} />}
      {tone === "company" && <path d="M6 0.5 12 11.5H0Z" fill={color} />}
      {tone === "mixed" && <rect x="0.75" y="0.75" width="10.5" height="10.5" fill="none" stroke={color} strokeWidth="1.5" />}
    </svg>
  );
}

/** The one way to name a vault: a pill in the vault's solid colour with its
 * shape before the name. The shape takes the chip's text colour, so it stays
 * visible on its own fill. The kind is also said in words for screen readers. */
export default function VaultChip({
  tone,
  name,
  className = "",
  title,
}: {
  tone: VaultTone;
  name: React.ReactNode;
  className?: string;
  title?: string;
}) {
  const on = tone === "mixed" ? "var(--ink)" : `var(--on-vault-${tone})`;
  return (
    <span className="vault-chip" data-tone={tone} title={title} style={{ color: on }}>
      <VaultShape tone={tone} size={8} fill={on} />
      <span className={`truncate ${className}`}>
        <span className="sr-only">{WORD[tone]}: </span>
        {name}
      </span>
    </span>
  );
}
