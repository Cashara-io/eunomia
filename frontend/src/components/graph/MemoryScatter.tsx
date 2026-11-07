"use client";

// The Memory map, flat: each memory is a dot at the first two coordinates of its place in the
// memory space (the server's projection), so similar memories sit close together. Click a dot, or
// focus it and press Enter, to pick it. Picked dots carry their comparison number.

import { useEffect, useRef, useState } from "react";

export type ScatterPoint = { id: string; x: number; y: number; color: string; label: string };

const PAD = 28; // px kept clear around the outermost dot
const R = 5;
const R_HOT = 7;

export default function MemoryScatter({
  points,
  selectedId,
  highlightIds,
  onSelect,
  ariaLabel,
}: {
  points: ScatterPoint[];
  selectedId?: string | null;
  /** ids in comparison order: dot n shows the number n + 1 */
  highlightIds: string[];
  onSelect: (id: string) => void;
  ariaLabel: string;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const [dims, setDims] = useState({ w: 640, h: 420 });
  const [hovered, setHovered] = useState<string | null>(null);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box && box.width >= 10 && box.height >= 10) setDims({ w: Math.round(box.width), h: Math.round(box.height) });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // One scale for both axes, so distances on screen keep their proportions.
  const xs = points.map((p) => p.x);
  const ys = points.map((p) => p.y);
  const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const spanX = Math.max(maxX - minX, 1e-6);
  const spanY = Math.max(maxY - minY, 1e-6);
  const scale = Math.min((dims.w - 2 * PAD) / spanX, (dims.h - 2 * PAD) / spanY);
  const offX = (dims.w - spanX * scale) / 2;
  const offY = (dims.h - spanY * scale) / 2;
  const single = points.length === 1;
  const sx = (x: number) => (single ? dims.w / 2 : offX + (x - minX) * scale);
  // screen y grows downwards; flip so "up" in the space is up on screen
  const sy = (y: number) => (single ? dims.h / 2 : dims.h - (offY + (y - minY) * scale));

  const order = new Map(highlightIds.map((id, i) => [id, i + 1]));
  // picked and hovered dots draw last, on top of their neighbours
  const drawn = [...points].sort((a, b) => Number(order.has(a.id) || a.id === selectedId) - Number(order.has(b.id) || b.id === selectedId));

  return (
    <div ref={wrapRef} className="absolute inset-0">
      <svg viewBox={`0 0 ${dims.w} ${dims.h}`} width="100%" height="100%" role="img" aria-label={ariaLabel}>
        {drawn.map((p) => {
          const n = order.get(p.id);
          const picked = p.id === selectedId;
          const hot = picked || hovered === p.id;
          const x = sx(p.x);
          const y = sy(p.y);
          return (
            <g
              key={p.id}
              transform={`translate(${x},${y})`}
              role="button"
              tabIndex={0}
              aria-label={p.label}
              aria-pressed={n !== undefined}
              style={{ cursor: "pointer" }}
              // a click picks without focusing, so no keyboard ring is left on the dot
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => onSelect(p.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onSelect(p.id);
                }
              }}
              onMouseEnter={() => setHovered(p.id)}
              onMouseLeave={() => setHovered((h) => (h === p.id ? null : h))}
            >
              <title>{p.label}</title>
              {/* a larger invisible target, so small dots are easy to hit */}
              <circle r={12} fill="transparent" />
              {n !== undefined ? (
                <>
                  <circle r={10} fill="var(--accent)" stroke="var(--canvas)" strokeWidth={2} />
                  <text
                    textAnchor="middle"
                    dominantBaseline="central"
                    fontSize={11}
                    fontWeight={600}
                    fontFamily="var(--font-sans), ui-sans-serif, system-ui, sans-serif"
                    fill="var(--on-accent)"
                    pointerEvents="none"
                  >
                    {n}
                  </text>
                </>
              ) : (
                <circle
                  r={hot ? R_HOT : R}
                  fill={p.color}
                  fillOpacity={hot ? 1 : 0.8}
                  stroke={picked ? "var(--ink)" : "var(--canvas)"}
                  strokeWidth={picked ? 2 : 1}
                />
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}
