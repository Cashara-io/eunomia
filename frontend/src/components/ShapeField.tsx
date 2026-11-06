"use client";

import { useEffect, useRef, type ReactNode } from "react";

/** One shape in a playable composition. (x, y) is its home in viewBox units and
 * the centre `draw` is drawn around; r is its round collider; a its resting angle. */
export type ShapeBody = { x: number; y: number; r: number; a?: number; draw: ReactNode };

type State = { x: number; y: number; vx: number; vy: number; a: number; va: number };

// Tuned by feel: a thrown shape flies, wobbles once and settles home in about a second.
const SPRING = 16;
const DAMP = 3.4;
const TURN_SPRING = 14;
const TURN_DAMP = 3;
const BOUNCE = 0.55;
const PUSH = 1400;
const PUSH_RANGE = 26;

/** A Bauhaus composition you can play with: grab a shape and throw it, brush
 * past one to nudge it. Shapes bump into each other and spring back to where
 * the composition put them, so the picture always comes back. The loop runs
 * only while something moves. Under reduced motion it is a still picture.
 * Decorative, so it is hidden from screen readers. */
export default function ShapeField({
  viewBox,
  preserveAspectRatio,
  bodies,
  className = "",
  overflow = false,
}: {
  viewBox: string;
  preserveAspectRatio: string;
  bodies: ShapeBody[];
  className?: string;
  /** Let thrown shapes leave the svg box (the parent still clips them). */
  overflow?: boolean;
}) {
  const svgRef = useRef<SVGSVGElement>(null);
  const nodes = useRef<(SVGGElement | null)[]>([]);

  useEffect(() => {
    const svg = svgRef.current;
    if (!svg || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    const s: State[] = bodies.map((b) => ({ x: b.x, y: b.y, vx: 0, vy: 0, a: b.a ?? 0, va: 0 }));
    const mass = bodies.map((b) => b.r * b.r);
    // Shapes that overlap in the composition are layered on purpose: never push those apart.
    const pairs: [number, number][] = [];
    for (let i = 0; i < bodies.length; i++)
      for (let j = i + 1; j < bodies.length; j++)
        if (Math.hypot(bodies[i].x - bodies[j].x, bodies[i].y - bodies[j].y) >= bodies[i].r + bodies[j].r) pairs.push([i, j]);

    let drag: { i: number; dx: number; dy: number; id: number; t: number } | null = null;
    let pointer: { x: number; y: number } | null = null;
    let frame = 0;
    let last = 0;

    const toSvg = (e: PointerEvent) => {
      const m = svg.getScreenCTM();
      if (!m) return null;
      const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m.inverse());
      return { x: p.x, y: p.y };
    };

    const paint = () => {
      s.forEach((b, i) => nodes.current[i]?.setAttribute("transform", `translate(${b.x} ${b.y}) rotate(${b.a})`));
    };

    const tick = (now: number) => {
      const dt = Math.min((now - last) / 1000, 1 / 30);
      last = now;
      let moving = drag !== null;

      s.forEach((b, i) => {
        if (drag?.i === i) return;
        const home = bodies[i];
        let ax = -SPRING * (b.x - home.x) - DAMP * b.vx;
        let ay = -SPRING * (b.y - home.y) - DAMP * b.vy;
        if (pointer && !drag) {
          const dx = b.x - pointer.x;
          const dy = b.y - pointer.y;
          const d = Math.hypot(dx, dy) || 1;
          const reach = home.r + PUSH_RANGE;
          if (d < reach) {
            const f = (PUSH * (1 - d / reach)) / Math.sqrt(home.r);
            ax += (dx / d) * f;
            ay += (dy / d) * f;
          }
        }
        b.vx += ax * dt;
        b.vy += ay * dt;
        b.x += b.vx * dt;
        b.y += b.vy * dt;
        b.va += (-TURN_SPRING * (b.a - (home.a ?? 0)) - TURN_DAMP * b.va) * dt;
        b.a += b.va * dt;
        if (Math.hypot(b.x - home.x, b.y - home.y) > 0.05 || Math.hypot(b.vx, b.vy) > 0.5 || Math.abs(b.va) > 0.5 || Math.abs(b.a - (home.a ?? 0)) > 0.05)
          moving = true;
      });

      // Collisions: round colliders, the dragged shape is immovable (the hand wins).
      for (const [i, j] of pairs) {
        const p = s[i];
        const q = s[j];
        const dx = q.x - p.x;
        const dy = q.y - p.y;
        const d = Math.hypot(dx, dy) || 0.001;
        const overlap = bodies[i].r + bodies[j].r - d;
        if (overlap <= 0) continue;
        const nx = dx / d;
        const ny = dy / d;
        const wi = drag?.i === i ? 0 : 1 / mass[i];
        const wj = drag?.i === j ? 0 : 1 / mass[j];
        const w = wi + wj;
        if (w === 0) continue;
        p.x -= nx * overlap * (wi / w);
        p.y -= ny * overlap * (wi / w);
        q.x += nx * overlap * (wj / w);
        q.y += ny * overlap * (wj / w);
        const closing = (q.vx - p.vx) * nx + (q.vy - p.vy) * ny;
        if (closing < 0) {
          const k = (-(1 + BOUNCE) * closing) / w;
          p.vx -= k * wi * nx;
          p.vy -= k * wi * ny;
          q.vx += k * wj * nx;
          q.vy += k * wj * ny;
          // a glancing hit sets a shape spinning
          const spin = ((q.vx - p.vx) * -ny + (q.vy - p.vy) * nx) * 0.6;
          p.va -= spin * wi * mass[i] * 0.02;
          q.va += spin * wj * mass[j] * 0.02;
        }
        moving = true;
      }

      paint();
      frame = moving || pointer ? requestAnimationFrame(tick) : 0;
      if (!frame) {
        // Settled: snap exactly home so the composition is pixel-identical to the still one.
        s.forEach((b, i) => Object.assign(b, { x: bodies[i].x, y: bodies[i].y, vx: 0, vy: 0, a: bodies[i].a ?? 0, va: 0 }));
        paint();
      }
    };

    const wake = () => {
      if (frame) return;
      last = performance.now();
      frame = requestAnimationFrame(tick);
    };

    const onDown = (e: PointerEvent) => {
      const g = (e.target as Element).closest<SVGGElement>("[data-body]");
      const p = toSvg(e);
      if (!g || !p || e.button !== 0) return;
      const i = Number(g.dataset.body);
      e.preventDefault();
      try {
        svg.setPointerCapture(e.pointerId);
      } catch {
        // the pointer is already gone; the drag still ends on pointerup or leave
      }
      drag = { i, dx: s[i].x - p.x, dy: s[i].y - p.y, id: e.pointerId, t: e.timeStamp };
      s[i].vx = s[i].vy = 0;
      svg.style.cursor = "grabbing";
      wake();
    };
    const onMove = (e: PointerEvent) => {
      const p = toSvg(e);
      if (!p) return;
      if (drag && e.pointerId === drag.id) {
        const b = s[drag.i];
        const dt = Math.max((e.timeStamp - drag.t) / 1000, 1 / 240);
        const nx = p.x + drag.dx;
        const ny = p.y + drag.dy;
        // Smoothed hand velocity, so a flick throws and a slow release drops.
        b.vx = b.vx * 0.5 + ((nx - b.x) / dt) * 0.5;
        b.vy = b.vy * 0.5 + ((ny - b.y) / dt) * 0.5;
        // Holding a shape off-centre swings it.
        b.va += ((-drag.dy * b.vx + drag.dx * b.vy) / (bodies[drag.i].r * bodies[drag.i].r)) * 0.4 * dt * 60;
        b.va = Math.max(-400, Math.min(400, b.va));
        b.a += b.va * dt;
        b.x = nx;
        b.y = ny;
        drag.t = e.timeStamp;
      } else if (e.pointerType === "mouse") {
        pointer = p;
      }
      wake();
    };
    const onUp = (e: PointerEvent) => {
      if (!drag || e.pointerId !== drag.id) return;
      // A hand that stopped before letting go drops the shape instead of throwing it.
      if (e.timeStamp - drag.t > 80) s[drag.i].vx = s[drag.i].vy = 0;
      drag = null;
      svg.style.cursor = "";
      if (svg.hasPointerCapture(e.pointerId)) svg.releasePointerCapture(e.pointerId);
      wake();
    };
    const onLeave = () => {
      pointer = null;
    };

    svg.addEventListener("pointerdown", onDown);
    svg.addEventListener("pointermove", onMove);
    svg.addEventListener("pointerup", onUp);
    svg.addEventListener("pointercancel", onUp);
    svg.addEventListener("pointerleave", onLeave);
    return () => {
      cancelAnimationFrame(frame);
      svg.removeEventListener("pointerdown", onDown);
      svg.removeEventListener("pointermove", onMove);
      svg.removeEventListener("pointerup", onUp);
      svg.removeEventListener("pointercancel", onUp);
      svg.removeEventListener("pointerleave", onLeave);
    };
  }, [bodies]);

  return (
    <svg
      ref={svgRef}
      viewBox={viewBox}
      preserveAspectRatio={preserveAspectRatio}
      className={`shape-field ${className}`}
      overflow={overflow ? "visible" : undefined}
      aria-hidden
    >
      {bodies.map((b, i) => (
        <g
          key={i}
          ref={(el) => {
            nodes.current[i] = el;
          }}
          data-body={i}
          transform={`translate(${b.x} ${b.y}) rotate(${b.a ?? 0})`}
        >
          <g className="shape-in" style={{ ["--i" as string]: i }}>
            {b.draw}
          </g>
        </g>
      ))}
    </svg>
  );
}
