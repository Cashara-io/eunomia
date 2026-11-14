"use client";
import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";
import styles from "./strata.module.css";

type Side = "l" | "r" | "t" | "b";
export type Wire = {
  id: string;
  from: string;
  to: string;
  fromSide: Side;
  toSide: Side;
  tone: "line" | "on" | "off" | "definition" | "tether";
  /** Stroke width in px, for weighted bands. */
  weight?: number;
  arrow?: boolean;
  /** Draw the arrowhead at both ends. */
  both?: boolean;
  label?: string;
};
type Drawn = { id: string; d: string; tone: Wire["tone"]; weight?: number; arrow: boolean; both: boolean; label?: string; lx: number; ly: number };

const anchor = (r: DOMRect, side: Side, o: DOMRect) => {
  const x = (side === "l" ? r.left : side === "r" ? r.right : r.left + r.width / 2) - o.left;
  const y = (side === "t" ? r.top : side === "b" ? r.bottom : r.top + r.height / 2) - o.top;
  return { x, y, side };
};
const lead = (p: { x: number; y: number; side: Side }, d: number) =>
  p.side === "l" ? { x: p.x - d, y: p.y } : p.side === "r" ? { x: p.x + d, y: p.y } : p.side === "t" ? { x: p.x, y: p.y - d } : { x: p.x, y: p.y + d };

/** Lines drawn between elements of the stage that carry data-wire. Measured from the DOM so they follow layout, scroll and resize. */
export default function Wires({ stage, wires, receipt }: { stage: RefObject<HTMLElement | null>; wires: Wire[]; receipt?: { from: string; inspector: string } }) {
  const [drawn, setDrawn] = useState<Drawn[]>([]);
  const last = useRef("");
  const find = (root: HTMLElement, id: string) =>
    [...root.querySelectorAll<HTMLElement>(`[data-wire="${CSS.escape(id)}"]`)].find((e) => e.getClientRects().length > 0);
  const measure = useCallback(() => {
    const root = stage.current;
    if (!root) return;
    const origin = root.getBoundingClientRect();
    if (receipt) {
      const card = root.querySelector<HTMLElement>("[data-receipt]");
      const row = find(root, receipt.from);
      if (card && row) {
        const top = row.getBoundingClientRect().top - origin.top - 20;
        card.style.top = `${Math.max(8, Math.min(top, origin.height - card.offsetHeight - 56))}px`;
      }
    }
    const next: Drawn[] = [];
    for (const w of wires) {
      const a = find(root, w.from);
      const b = find(root, w.to);
      if (!a || !b) continue;
      const p = anchor(a.getBoundingClientRect(), w.fromSide, origin);
      const q = anchor(b.getBoundingClientRect(), w.toSide, origin);
      const reach = Math.max(28, Math.hypot(q.x - p.x, q.y - p.y) / 2.5);
      const c1 = lead(p, reach);
      const c2 = lead(q, reach);
      next.push({
        id: w.id,
        d: `M${p.x.toFixed(1)} ${p.y.toFixed(1)}C${c1.x.toFixed(1)} ${c1.y.toFixed(1)} ${c2.x.toFixed(1)} ${c2.y.toFixed(1)} ${q.x.toFixed(1)} ${q.y.toFixed(1)}`,
        tone: w.tone,
        weight: w.weight,
        arrow: !!w.arrow,
        both: !!w.both,
        label: w.label,
        lx: (p.x + q.x) / 2,
        ly: (p.y + q.y) / 2,
      });
    }
    const key = JSON.stringify(next);
    if (key === last.current) return;
    last.current = key;
    setDrawn(next);
  }, [stage, wires, receipt]);
  useLayoutEffect(() => {
    measure();
    const root = stage.current;
    if (!root) return;
    let frame = 0;
    const again = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    const observer = new ResizeObserver(again);
    observer.observe(root);
    root.addEventListener("scroll", again, true);
    window.addEventListener("resize", again);
    void document.fonts?.ready.then(again);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      root.removeEventListener("scroll", again, true);
      window.removeEventListener("resize", again);
    };
  }, [measure, stage]);
  return (
    <svg className={styles.wires} aria-hidden="true" focusable="false">
      <defs>
        <marker id="strata-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="8" markerHeight="8" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
          <path d="M0 0L8 4L0 8z" fill="currentColor" />
        </marker>
      </defs>
      {drawn.map((w) => (
        <path
          key={w.id}
          d={w.d}
          className={styles.wire}
          data-tone={w.tone}
          style={w.weight ? { strokeWidth: w.weight } : undefined}
          markerEnd={w.arrow ? "url(#strata-arrow)" : undefined}
          markerStart={w.both ? "url(#strata-arrow)" : undefined}
        />
      ))}
    </svg>
  );
}
