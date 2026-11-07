/** Eunomia's mark: the shape trio. A red circle, a yellow square and a blue
 * triangle in a row, one unit apart: the personal, project and company
 * vaults as one mark. Flat brand fills (the Bauhaus red, yellow and blue): the
 * mark keeps its colours even where Personal is drawn grey. `size` is the footprint height the old tile used (20 in the
 * sidebar, 28 on auth and onboarding); the shapes sit at 60% of it. */
export default function EunomiaMark({ size = 20, className }: { size?: number; className?: string }) {
  const h = Math.round(size * 0.6);
  const w = Math.round((h * 44) / 12);
  return (
    <svg width={w} height={h} viewBox="0 0 44 12" fill="none" aria-hidden className={className} style={{ flexShrink: 0 }}>
      <circle cx="6" cy="6" r="6" fill="var(--vault-personal)" />
      <rect x="16" y="0" width="12" height="12" fill="var(--vault-project)" />
      <path d="M38 0 44 12H32Z" fill="var(--vault-company)" />
    </svg>
  );
}
