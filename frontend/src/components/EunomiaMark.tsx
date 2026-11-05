/** Eunomia's mark: a canvas tile where three scattered points resolve into
 * one -- disparate agents and sources -> one shared memory. The centre node
 * sits where a cursor's tip would land. Asymmetric on purpose. */
export default function EunomiaMark({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden>
      <rect width="24" height="24" rx="4" fill="var(--accent)" />
      <g stroke="var(--on-accent)" strokeWidth="1.5" strokeLinecap="round" opacity="0.55">
        <line x1="12.5" y1="12.5" x2="6.5" y2="7" />
        <line x1="12.5" y1="12.5" x2="18" y2="8.5" />
        <line x1="12.5" y1="12.5" x2="9" y2="18.5" />
      </g>
      <g fill="var(--on-accent)">
        <circle cx="6.5" cy="7" r="1.6" opacity="0.6" />
        <circle cx="18" cy="8.5" r="1.3" opacity="0.6" />
        <circle cx="9" cy="18.5" r="1.8" opacity="0.6" />
        <circle cx="12.5" cy="12.5" r="2.9" />
      </g>
    </svg>
  );
}
