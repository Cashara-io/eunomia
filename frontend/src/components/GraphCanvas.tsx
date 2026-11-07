"use client";

// The entity graph's canvas: a flat SVG with a live d3-force layout. Drag a
// node and the links follow; release and physics settles it back. Drag the
// background to pan, scroll or use the buttons to zoom, click (or Enter or Space) a node
// to select it.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, type Simulation, type SimulationNodeDatum } from "d3-force";
import { ArrowCounterClockwiseIcon, ArrowsOutIcon, EyeIcon, EyeSlashIcon, MinusIcon, PlusIcon } from "@phosphor-icons/react/ssr";
import { SvgMonogram } from "./graph/Monogram";
import Tooltip, { TooltipGroup } from "./bits/Tooltip";
import type { EntityKind } from "@/lib/types";

export type GraphNode = { id: string; kind: EntityKind; name: string; owner_email: string | null };
export type GraphEdge = { source: string; target: string; label: string };
export type CanvasInsets = { top: number; right: number; bottom: number; left: number };

type Sim = SimulationNodeDatum & GraphNode;
type View = { zoom: number; x: number; y: number };

const DRAG_THRESHOLD = 4; // px before a pointerdown counts as a drag, not a click
// low enough that a large graph still fits a phone screen
const ZOOM_MIN = 0.2;
const ZOOM_MAX = 2.5;
const ZOOM_STEP = 0.2;
const FIT_PADDING = 48; // screen px kept clear around the outermost node or label
const FIT_ZOOM_MAX = 1.4; // an auto-fit never blows a small graph up past this
// Labels stay 12px on screen at any zoom (the font is divided by the zoom); they sit LABEL_Y layout
// units under the node's centre.
const LABEL_FONT = 12;
const LABEL_Y = 26;
const LABEL_MAX = 18; // characters before the label is cut with an ellipsis
const NODE_R = 16;
const labelText = (name: string) => (name.length > LABEL_MAX ? `${name.slice(0, LABEL_MAX - 1)}…` : name);
// screen px from a node's centre to its label's edge: about 6.5px per character of 12px UI sans
const labelHalfWidth = (name: string) => (labelText(name).length * 6.5) / 2;
const PRE_TICKS = 120; // layout steps run before the first frame, so the first fit is close to the settled shape

type Box = { left: number; right: number; top: number; bottom: number };
// The on-screen box (relative to an arbitrary origin) of every node and its label at `zoom`.
// Shapes scale with the zoom, labels do not, so the box is measured per zoom.
function screenBox(pts: { x: number; y: number; half: number }[], zoom: number): Box {
  const box = { left: Infinity, right: -Infinity, top: Infinity, bottom: -Infinity };
  for (const p of pts) {
    const sx = p.x * zoom;
    const sy = p.y * zoom;
    const half = Math.max(p.half, NODE_R * zoom);
    box.left = Math.min(box.left, sx - half);
    box.right = Math.max(box.right, sx + half);
    box.top = Math.min(box.top, sy - NODE_R * zoom);
    box.bottom = Math.max(box.bottom, sy + LABEL_Y * zoom + 4); // baseline plus descenders
  }
  return box;
}

// Zoom/pan that centres `nodes` and their labels inside the part of the w x h canvas the floating
// UI leaves free, with FIT_PADDING all round. Shared by auto-fit and the Fit button.
function computeFit(nodes: { x?: number; y?: number; name: string }[], w: number, h: number, ins: CanvasInsets): View {
  const pts = nodes
    .filter((n): n is { x: number; y: number; name: string } => typeof n.x === "number" && typeof n.y === "number")
    .map((n) => ({ x: n.x, y: n.y, half: labelHalfWidth(n.name) }));
  if (pts.length === 0) return { zoom: 1, x: 0, y: 0 };
  const freeW = Math.max(w - ins.left - ins.right, 100);
  const freeH = Math.max(h - ins.top - ins.bottom, 100);
  const roomW = Math.max(freeW - 2 * FIT_PADDING, 40);
  const roomH = Math.max(freeH - 2 * FIT_PADDING, 40);
  const fits = (z: number) => {
    const b = screenBox(pts, z);
    return b.right - b.left <= roomW && b.bottom - b.top <= roomH;
  };
  // the box grows with the zoom, so the largest zoom that fits is found by bisection
  let zoom = ZOOM_MIN;
  if (fits(FIT_ZOOM_MAX)) zoom = FIT_ZOOM_MAX;
  else if (fits(ZOOM_MIN)) {
    let lo = ZOOM_MIN;
    let hi = FIT_ZOOM_MAX;
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (fits(mid)) lo = mid;
      else hi = mid;
    }
    zoom = lo;
  }
  const b = screenBox(pts, zoom);
  const cx = ins.left + freeW / 2;
  const cy = ins.top + freeH / 2;
  // screen = w/2 + zoom * (p - w/2 + view.x) = zoom * p + w/2 + zoom * (view.x - w/2); solve for the
  // view that puts the middle of the box on (cx, cy)
  return {
    zoom,
    x: (cx - (b.left + b.right) / 2 - w / 2) / zoom + w / 2,
    y: (cy - (b.top + b.bottom) / 2 - h / 2) / zoom + h / 2,
  };
}

function ToolButton({ label, icon, run }: { label: string; icon: React.ReactNode; run: () => void }) {
  return (
    <Tooltip label={label}>
      <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label={label} onClick={run}>
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
  chrome,
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
  /** the page's "Hide controls" switch, drawn in the canvas corner beside the zoom buttons */
  chrome?: { hidden: boolean; toggle: () => void };
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
  const visibleRef = useRef(visibleKinds);
  // dimsRef is set where the size is measured (below), so the first layout already uses the real size.
  useEffect(() => {
    insetsRef.current = insets;
    visibleRef.current = visibleKinds;
  });
  // Only the nodes on screen count for a fit: a hidden kind must not pull the camera away.
  const fitNow = (all: Sim[]) =>
    setView(computeFit(all.filter((n) => visibleRef.current.has(n.kind)), dimsRef.current.w, dimsRef.current.h, insetsRef.current));

  // True until the person pans, zooms or drags. While true the camera keeps every visible node in
  // the free area: as the layout settles, and when that area or the shown kinds change.
  const autoFit = useRef(true);
  const visibleKey = [...visibleKinds].sort().join(",");
  // Opening or closing the inspector changes the free area: fit again even after a pan or zoom.
  const lastRight = useRef(insets.right);
  useEffect(() => {
    if (lastRight.current === insets.right) return;
    lastRight.current = insets.right;
    autoFit.current = true;
  }, [insets.right]);
  useEffect(() => {
    if (!autoFit.current) return;
    const s = simRef.current;
    if (s) fitNow(s.nodes());
  }, [dims.w, dims.h, insets, visibleKey]);

  const zoomBy = (f: number) => {
    autoFit.current = false;
    setView((v) => ({ ...v, zoom: Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, v.zoom * f)) }));
  };

  // Track the canvas size so the viewBox and the centre force follow a resize. Measured once before
  // the first paint too: a first layout and fit at the 640x420 placeholder size, corrected only once
  // the observer reported, left the nodes crowding one side of a wide canvas.
  useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const apply = (width: number, height: number) => {
      if (width < 10 || height < 10) return;
      const next = { w: Math.round(width), h: Math.round(height) };
      dimsRef.current = next;
      setDims((prev) => (prev.w === next.w && prev.h === next.h ? prev : next));
    };
    const box = el.getBoundingClientRect();
    apply(box.width, box.height);
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r) apply(r.width, r.height);
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
      .on("tick", () => {
        setNodes([...s.nodes()]);
        // follow the layout as it settles, so a reload or vault switch never leaves nodes off screen
        if (autoFit.current) fitNow(s.nodes());
      })
      // one last fit on the settled shape, with whatever insets and size hold by then
      .on("end", () => {
        if (autoFit.current) fitNow(s.nodes());
      });
    s.tick(PRE_TICKS);
    setNodes([...s.nodes()]);
    fitNow(s.nodes());
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
    fitNow(nodes);
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
    e.preventDefault(); // no text selection while dragging across the page's controls
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
          e.preventDefault(); // panning across the header must not select its text
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
            const ring = picked || hot ? "var(--accent)" : "none";
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
                  <SvgMonogram name={n.name || n.id} r={r} color={color} ring={ring} ringWidth={shape.strokeWidth} />
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
              </g>
            );
          })}
          {/* Labels last, over every edge and shape, with a canvas-coloured halo so no line runs through text. */}
          <g pointerEvents="none" aria-hidden>
            {visible.map((n) => {
              const lit = hovered === n.id || draggingId === n.id || selectedId === n.id;
              return (
                <text
                  key={n.id}
                  x={n.x}
                  y={(n.y ?? 0) + LABEL_Y}
                  textAnchor="middle"
                  fontSize={LABEL_FONT / view.zoom}
                  fontFamily="var(--font-sans), ui-sans-serif, system-ui, sans-serif"
                  fill={lit ? "var(--ink)" : "var(--ink-dim)"}
                  stroke="var(--canvas)"
                  strokeWidth={4 / view.zoom}
                  strokeLinejoin="round"
                  paintOrder="stroke"
                >
                  {labelText(n.name)}
                </text>
              );
            })}
          </g>
        </g>
      </svg>

      <TooltipGroup>
        <div className="panel p-1 absolute bottom-4 flex gap-1" style={{ right: insets.right ? insets.right + 8 : 16 }}>
          <ToolButton label="Zoom in" icon={<PlusIcon weight="duotone" size={16} />} run={() => zoomBy(1 + ZOOM_STEP)} />
          <ToolButton label="Zoom out" icon={<MinusIcon weight="duotone" size={16} />} run={() => zoomBy(1 - ZOOM_STEP)} />
          <ToolButton label="Fit to view" icon={<ArrowsOutIcon weight="duotone" size={16} />} run={fitToView} />
          <ToolButton label="Reset view" icon={<ArrowCounterClockwiseIcon weight="duotone" size={16} />} run={resetView} />
          {chrome && (
            <ToolButton
              label={chrome.hidden ? "Show controls" : "Hide controls"}
              icon={chrome.hidden ? <EyeIcon weight="duotone" size={16} /> : <EyeSlashIcon weight="duotone" size={16} />}
              run={chrome.toggle}
            />
          )}
        </div>
      </TooltipGroup>
      <p className="label absolute left-4 sm:left-8 lg:left-12 bottom-4 hidden md:block pointer-events-none">Drag to rearrange · Scroll to zoom · Click a name to open it</p>
    </div>
  );
}
