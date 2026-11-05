// An animated copy of EunomiaMark (same geometry, see components/EunomiaMark.tsx)
// for the onboarding welcome: the three points glide in to their places and
// their links draw into the centre node, so many points resolve into one.
// 260ms per point, 50ms stagger; a first-run moment. `still` renders the mark at rest.
import "@/components/bits/bits.css";

const C = { x: 12.5, y: 12.5 };
const POINTS = [
  { x: 6.5, y: 7, r: 1.6 },
  { x: 18, y: 8.5, r: 1.3 },
  { x: 9, y: 18.5, r: 1.8 },
];

export default function ResolveMark({ size = 36, still = false }: { size?: number; still?: boolean }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden className={still ? undefined : "resolve-mark"}>
      <style>{`
        @keyframes resolve-point { from { transform: translate(var(--dx), var(--dy)); opacity: 0.001; } }
        @keyframes resolve-core { from { transform: scale(0.6); opacity: 0.001; } }
        .resolve-mark .resolve-point { animation: resolve-point 260ms var(--ease-out) both; animation-delay: calc(var(--i) * 50ms); }
        .resolve-mark .resolve-core { transform-box: fill-box; transform-origin: center; animation: resolve-core 260ms var(--ease-out) both; }
        .resolve-mark .bits-draw { --bits-draw-ms: 200ms; }
        @media (prefers-reduced-motion: reduce) { .resolve-mark .resolve-point, .resolve-mark .resolve-core { animation: none; } }
      `}</style>
      <rect width="24" height="24" rx="4" fill="var(--accent)" />
      <g stroke="var(--on-accent)" strokeWidth="1.5" strokeLinecap="round" opacity="0.55">
        {POINTS.map((p, i) => (
          // Drawn from the outer point toward the centre, after that point lands.
          <line
            key={i}
            x1={p.x}
            y1={p.y}
            x2={C.x}
            y2={C.y}
            pathLength={1}
            className="bits-draw"
            data-animate={still ? undefined : ""}
            style={{ animationDelay: `${160 + i * 50}ms` }}
          />
        ))}
      </g>
      <g fill="var(--on-accent)">
        {POINTS.map((p, i) => (
          <circle
            key={i}
            cx={p.x}
            cy={p.y}
            r={p.r}
            opacity="0.6"
            className="resolve-point"
            // Start 60% further out from the centre than the resting place.
            style={{ ["--dx" as string]: `${(p.x - C.x) * 0.6}px`, ["--dy" as string]: `${(p.y - C.y) * 0.6}px`, ["--i" as string]: i }}
          />
        ))}
        <circle cx={C.x} cy={C.y} r="2.9" className="resolve-core" />
      </g>
    </svg>
  );
}
