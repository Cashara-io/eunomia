"use client";

// The entity graph's canvas: a flat SVG with a live d3-force layout. Drag a
// node and the links follow; release and physics settles it back. Drag the
// background to pan, scroll or use the buttons to zoom, click (or Enter or Space) a node
// to select it. The vector cloud stays 3D (Scene3D).

import { useEffect, useRef, useState } from "react";
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, type Simulation, type SimulationNodeDatum } from "d3-force";
import { Maximize2, Minus, Plus, RotateCcw } from "lucide-react";
import { Blobatar } from "@blobatar/react";
import Tooltip, { TooltipGroup } from "./bits/Tooltip";
import type { EntityKind } from "@/lib/types";

export type GraphNode = { id: string; kind: EntityKind; name: string; owner_email: string | null };
export type GraphEdge = { source: string; target: string; label: string };
export type CanvasInsets = { top: number; right: number; bottom: number; left: number };

type Sim = SimulationNodeDatum & GraphNode;
type View = { zoom: number; x: number; y: number };

const DRAG_THRESHOLD = 4; // px before a pointerdown counts as a drag, not a click
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2.5;
const ZOOM_STEP = 0.2;
const FIT_PADDING = 50;

// Zoom/pan that centres `nodes` inside the part of the w x h canvas the
// floating UI leaves free. Shared by auto-fit and the Fit button.
function computeFit(nodes: { x?: number; y?: number }[], w: number, h: number, ins: CanvasInsets): View {
  const pts = nodes.filter((n): n is { x: number; y: number } => typeof n.x === "number" && typeof n.y === "number");
  if (pts.length === 0) return { zoom: 1, x: 0, y: 0 };
  const xs = pts.map((p) => p.x);
  const ys = pts.map((p) => p.y);
  const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const freeW = Math.max(w - ins.left - ins.right, 100);
  const freeH = Math.max(h - ins.top - ins.bottom, 100);
  const fit = Math.min((freeW - 2 * FIT_PADDING) / Math.max(maxX - minX, 1), (freeH - 2 * FIT_PADDING) / Math.max(maxY - minY, 1));
  const zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, fit));
  const cx = ins.left + freeW / 2;
  const cy = ins.top + freeH / 2;
  // screen = w/2 + zoom * (p - w/2 + view.x), so solve for the view that puts the box centre on (cx, cy)
  return { zoom, x: (cx - w / 2) / zoom + w / 2 - (minX + maxX) / 2, y: (cy - h / 2) / zoom + h / 2 - (minY + maxY) / 2 };
}

function ToolButton({ label, icon, run }: { label: string; icon: React.ReactNode; run: () => void }) {
  return (
    <Tooltip label={label}>
      <button type="button" className="btn btn-ghost btn-sm btn-icon w-[26px]" aria-label={label} onClick={run}>
        {icon}
      </button>
    </Tooltip>
  );
}

export default function GraphCanvas({
  nodes: inNodes,
  edges,
  kindColor,
  visibleKinds,
  selectedId,
  onSelect,
  insets,
  myEmail,
  ariaLabel,
}: {
  nodes: GraphNode[];
  edges: GraphEdge[];
  kindColor: (kind: EntityKind) => string;
  visibleKinds: Set<EntityKind>;
  selectedId?: string | null;
  onSelect: (id: string) => void;
  insets: CanvasInsets;
  myEmail: string | null;
  ariaLabel: string;
}) {
  const [nodes, setNodes] = useState<Sim[]>([]);
  const [view, setView] = useState<View>({ zoom: 1, x: 0, y: 0 });
  const [dims, setDims] = useState({ w: 640, h: 420 });
  const [hovered, setHovered] = useState<string | null>(null);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const [panning, setPanning] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const simRef = useRef<Simulation<Sim, undefined> | null>(null);
  const dragRef = useRef<{ id: string; moved: boolean } | null>(null);
  const panRef = useRef<{ startX: number; startY: number; originX: number; originY: number } | null>(null);
  const insetsRef = useRef(insets);
  const dimsRef = useRef(dims);
  useEffect(() => {
    insetsRef.current = insets;
    dimsRef.current = dims;
  });

  // True until the person pans, zooms or drags: then a change in the free area (the toolbar wrapping, a resize) re-fits.
  const autoFit = useRef(true);
  useEffect(() => {
    if (!autoFit.current) return;
    const s = simRef.current;
    if (s) setView(computeFit(s.nodes(), dims.w, dims.h, insets));
  }, [dims.w, dims.h, insets]);

  const zoomBy = (f: number) => {
    autoFit.current = false;
    setView((v) => ({ ...v, zoom: Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, v.zoom * f)) }));
  };

  // Track the canvas size so the viewBox and the centre force follow a resize.
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

  // The simulation runs live (not a one-shot layout) so a released node settles back in.
  useEffect(() => {
    autoFit.current = true;
    const sim: Sim[] = inNodes.map((n) => ({ ...n }));
    const byId = new Map(sim.map((n) => [n.id, n]));
    const links = edges.flatMap((e) => {
      const source = byId.get(e.source);
      const target = byId.get(e.target);
      return source && target ? [{ source, target }] : [];
    });
    const s = forceSimulation(sim)
      .force("link", forceLink(links).distance(90).strength(0.5))
      .force("charge", forceManyBody().strength(-160))
      .force("center", forceCenter(dimsRef.current.w / 2, dimsRef.current.h / 2))
      .force("collide", forceCollide(26))
      .on("tick", () => setNodes([...s.nodes()]))
      // fit once the layout settles, so a reload or vault switch never leaves nodes off screen
      .on("end", () => autoFit.current && setView(computeFit(s.nodes(), dimsRef.current.w, dimsRef.current.h, insetsRef.current)));
    simRef.current = s;
    return () => {
      s.stop();
      simRef.current = null;
    };
  }, [inNodes, edges]);

  useEffect(() => {
    simRef.current?.force("center", forceCenter(dims.w / 2, dims.h / 2)).alpha(0.3).restart();
  }, [dims.w, dims.h]);

  // Wheel zoom needs a non-passive listener to stop the page scrolling.
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      zoomBy(e.deltaY < 0 ? 1.1 : 0.9);
    };
    svg.addEventListener("wheel", onWheel, { passive: false });
    return () => svg.removeEventListener("wheel", onWheel);
  }, []);

  function fitToView() {
    autoFit.current = true;
    setView(computeFit(nodes.filter((n) => visibleKinds.has(n.kind)), dims.w, dims.h, insets));
  }

  function resetView() {
    autoFit.current = false;
    setView({ zoom: 1, x: 0, y: 0 });
  }

  function svgPoint(e: React.PointerEvent): { x: number; y: number } {
    const ctm = svgRef.current?.getScreenCTM();
    if (!svgRef.current || !ctm) return { x: 0, y: 0 };
    const pt = svgRef.current.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    const p = pt.matrixTransform(ctm.inverse());
    // undo the view transform so a dragged node lands under the pointer at any zoom/pan
    const { w, h } = dimsRef.current;
    return { x: (p.x - w / 2) / view.zoom + w / 2 - view.x, y: (p.y - h / 2) / view.zoom + h / 2 - view.y };
  }

  function nodeDown(e: React.PointerEvent, n: Sim) {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    dragRef.current = { id: n.id, moved: false };
    setDraggingId(n.id);
    n.fx = n.x;
    n.fy = n.y;
    simRef.current?.alphaTarget(0.3).restart();
  }

  function nodeMove(e: React.PointerEvent, n: Sim) {
    const d = dragRef.current;
    if (!d || d.id !== n.id) return;
    const { x, y } = svgPoint(e);
    if (!d.moved && (Math.abs(x - (n.fx ?? x)) > DRAG_THRESHOLD || Math.abs(y - (n.fy ?? y)) > DRAG_THRESHOLD)) {
      d.moved = true;
      autoFit.current = false;
    }
    n.fx = x;
    n.fy = y;
    setNodes((prev) => [...prev]);
  }

  function nodeUp(n: Sim) {
    const d = dragRef.current;
    if (!d || d.id !== n.id) return;
    n.fx = null;
    n.fy = null;
    simRef.current?.alphaTarget(0);
    dragRef.current = null;
    setDraggingId(null);
    if (!d.moved) onSelect(n.id);
  }

  const visible = nodes.filter((n) => visibleKinds.has(n.kind));
  const byId = new Map(visible.map((n) => [n.id, n]));
  const lines = edges.flatMap((e) => {
    const s = byId.get(e.source);
    const t = byId.get(e.target);
    return s && t ? [{ s, t }] : [];
  });

  return (
    <div ref={wrapRef} className="absolute inset-0">
      <svg
        ref={svgRef}
        viewBox={`0 0 ${dims.w} ${dims.h}`}
        width="100%"
        height="100%"
        role="img"
        aria-label={ariaLabel}
        style={{ touchAction: "none", userSelect: "none", WebkitUserSelect: "none", cursor: panning ? "grabbing" : "default" }}
        onPointerDown={(e) => {
          if (e.target !== e.currentTarget) return; // a node handled its own pointerdown
          e.currentTarget.setPointerCapture(e.pointerId);
          autoFit.current = false;
          panRef.current = { startX: e.clientX, startY: e.clientY, originX: view.x, originY: view.y };
          setPanning(true);
        }}
        onPointerMove={(e) => {
          const p = panRef.current;
          if (p) setView((v) => ({ ...v, x: p.originX + (e.clientX - p.startX) / v.zoom, y: p.originY + (e.clientY - p.startY) / v.zoom }));
        }}
        onPointerUp={() => {
          panRef.current = null;
          setPanning(false);
        }}
        onPointerCancel={() => {
          panRef.current = null;
          setPanning(false);
        }}
      >
        <g transform={`translate(${dims.w / 2},${dims.h / 2}) scale(${view.zoom}) translate(${-dims.w / 2 + view.x},${-dims.h / 2 + view.y})`}>
          {lines.map(({ s, t }, i) => (
            <line key={i} x1={s.x} y1={s.y} x2={t.x} y2={t.y} stroke="var(--border-strong)" strokeWidth={1} />
          ))}
          {visible.map((n) => {
            const hot = hovered === n.id || draggingId === n.id;
            const picked = selectedId === n.id;
            const r = hot || picked ? 16 : 14;
            const color = kindColor(n.kind);
            const ring = picked ? "var(--accent)" : hot ? "var(--felt)" : "none";
            const shape = { fill: color, stroke: ring, strokeWidth: picked || hot ? 2 : 0, opacity: picked || hot ? 1 : 0.85 };
            return (
              <g
                key={n.id}
                transform={`translate(${n.x},${n.y})`}
                tabIndex={0}
                role="button"
                aria-label={n.name}
                style={{ cursor: draggingId === n.id ? "grabbing" : "grab", touchAction: "none" }}
                onPointerDown={(e) => nodeDown(e, n)}
                onPointerMove={(e) => nodeMove(e, n)}
                onPointerUp={() => nodeUp(n)}
                onPointerCancel={() => nodeUp(n)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault(); // Space would scroll the page
                    onSelect(n.id);
                  }
                }}
                onMouseEnter={() => setHovered(n.id)}
                onMouseLeave={() => setHovered((h) => (h === n.id ? null : h))}
              >
                <title>{`${n.name}, added by ${!n.owner_email ? "unknown" : n.owner_email === myEmail ? "you" : n.owner_email}`}</title>
                {n.kind === "person" ? (
                  <>
                    {(hot || picked) && <circle r={r + 1} fill="none" stroke={ring} strokeWidth={2} />}
                    <foreignObject x={-r} y={-r} width={r * 2} height={r * 2} style={{ overflow: "visible" }}>
                      <Blobatar name={n.name || n.id} animate="hover" size={r * 2} />
                    </foreignObject>
                  </>
                ) : n.kind === "organisation" ? (
                  <rect x={-r * 0.82} y={-r * 0.82} width={r * 1.64} height={r * 1.64} rx={4} {...shape} />
                ) : n.kind === "location" ? (
                  <path
                    d={`M0,${-r * 1.15} C${r * 0.75},${-r * 1.15} ${r * 0.95},${-r * 0.2} 0,${r * 1.05} C${-r * 0.95},${-r * 0.2} ${-r * 0.75},${-r * 1.15} 0,${-r * 1.15} Z`}
                    {...shape}
                  />
                ) : n.kind === "repository" ? (
                  <rect x={-r * 0.95} y={-r * 0.95} width={r * 1.9} height={r * 1.9} rx={6} {...shape} />
                ) : n.kind === "file" ? (
                  <rect x={-r * 0.6} y={-r * 0.6} width={r * 1.2} height={r * 1.2} rx={1.5} {...shape} />
                ) : n.kind === "symbol" ? (
                  <rect x={-r * 0.62} y={-r * 0.62} width={r * 1.24} height={r * 1.24} {...shape} transform="rotate(45)" />
                ) : (
                  // an agent's own kind
                  <circle r={r * 0.7} {...shape} />
                )}
                <text
                  y={24}
                  textAnchor="middle"
                  fontSize={10.5}
                  fontFamily="var(--font-mono), ui-monospace, monospace"
                  fill={hot || picked ? "var(--ink)" : "var(--ink-dim)"}
                >
                  {n.name.length > 16 ? `${n.name.slice(0, 15)}…` : n.name}
                </text>
              </g>
            );
          })}
        </g>
      </svg>

      <TooltipGroup>
        <div className="panel p-0.5 absolute right-4 md:right-[25.5rem] bottom-4 flex gap-0.5">
          <ToolButton label="Zoom in" icon={<Plus size={13} strokeWidth={1.75} />} run={() => zoomBy(1 + ZOOM_STEP)} />
          <ToolButton label="Zoom out" icon={<Minus size={13} strokeWidth={1.75} />} run={() => zoomBy(1 - ZOOM_STEP)} />
          <ToolButton label="Fit to view" icon={<Maximize2 size={13} strokeWidth={1.75} />} run={fitToView} />
          <ToolButton label="Reset view" icon={<RotateCcw size={13} strokeWidth={1.75} />} run={resetView} />
        </div>
      </TooltipGroup>
      <p className="label absolute left-10 bottom-4 hidden md:block pointer-events-none">Drag to rearrange, scroll to zoom, click a node to open it</p>
    </div>
  );
}
