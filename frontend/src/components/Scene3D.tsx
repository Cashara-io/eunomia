"use client";

// One small three.js scene shared by the entity graph and the vector cloud:
// flat unlit discs (instanced, cheap for thousands of points) with a hairline
// ring, optional link lines and labelled axes, orbit/zoom, hover tooltip,
// click to select. Coordinates can be any scale: they're fitted into a unit
// sphere. An unlit sphere renders as a flat circle from every angle, so the
// world reads flat (Figma/tldraw) while orbit still works.

import { useEffect, useRef, useState } from "react";
import { Maximize, Minus, Plus } from "lucide-react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import Tooltip, { TooltipGroup } from "./bits/Tooltip";
import { cssVar, prefersReducedMotion } from "./bits/motion";

export type ScenePoint = { id: string; x: number; y: number; z: number; color: string; label: string; size?: number };
export type SceneLink = { source: string; target: string };
export type SceneInsets = { top: number; right: number; bottom: number; left: number };

type Props = {
  points: ScenePoint[];
  links?: SceneLink[];
  axes?: [string, string, string];
  selectedId?: string | null;
  /** points to mark with a numbered frame (a multi-selection), in number order */
  highlightIds?: string[];
  onSelect?: (id: string) => void;
  ariaLabel: string;
  /** draw every point's label (only sensible for small graphs) */
  labels?: boolean;
  /** floating zoom out / level / zoom in / reset bar, bottom right */
  zoomControls?: boolean;
  /** face-on camera, no auto-rotate, fitted to the node bounds inside `insets` */
  flat?: boolean;
  /** px covered by floating UI; the fit keeps nodes and labels clear of them */
  insets?: SceneInsets;
};

const CLICK_SLOP = 4; // px a pointer may move and still count as a click
const MAX_LABELS = 80;

// "var(--kind-person)" -> the computed color; three can't read CSS variables.
// Resolved per theme: a cache lives only for one theme pass.
function cssColor(c: string, cache?: Map<string, THREE.Color>): THREE.Color {
  const hit = cache?.get(c);
  if (hit) return hit;
  const m = c.match(/^var\((--[^)]+)\)$/);
  const value = m ? getComputedStyle(document.documentElement).getPropertyValue(m[1]).trim() : c;
  const color = new THREE.Color();
  try {
    color.setStyle(value || "gray");
  } catch {
    color.set(0x888888);
  }
  cache?.set(c, color);
  return color;
}

const START = new THREE.Vector3(1.8, 1.3, 1.8);
const NO_INSETS: SceneInsets = { top: 0, right: 0, bottom: 0, left: 0 };
const LABEL_H = 15; // px per label row, for collision nudging
const SELECTED_SCALE = 1.3;

// Motion for pointer work only (keyboard selection and zoom stay instant).
const PICK_MS = 260; // pick ring: scale 1 to 2.2 and fade
const RETICLE_IN_MS = 140; // reticle enter from scale(1.5)
const RETICLE_MOVE_MS = 120; // reticle glide between nodes
const EDGE_MS = 200; // incident edges brighten, the rest dim
const SETTLE_MS = 200; // zoom scrub springs back inside its limits
// JS stand-ins for --ease-out / --ease-in-out, for per-frame interpolation.
const easeOut = (t: number) => 1 - (1 - t) ** 4;
const easeInOut = (t: number) => (t < 0.5 ? 8 * t ** 4 : 1 - (-2 * t + 2) ** 4 / 2);
type Box = { x0: number; y0: number; x1: number; y1: number };
const pointerDriven = () => document.documentElement.dataset.input === "pointer" && !prefersReducedMotion();

export default function Scene3D({ points, links = [], axes, selectedId, highlightIds, onSelect, ariaLabel, labels, zoomControls, flat, insets }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const overlayRef = useRef<HTMLDivElement>(null);
  const zoomTextRef = useRef<HTMLSpanElement>(null);
  const apiRef = useRef<{
    zoom: (factor: number) => void;
    reset: () => void;
    refit: () => void;
    scrub: (dx: number, rate: number) => void;
    scrubEnd: () => void;
  } | null>(null);
  const scrubRef = useRef<{ id: number; x: number } | null>(null);
  const insetsRef = useRef<SceneInsets>(insets ?? NO_INSETS);
  const onSelectRef = useRef(onSelect);
  const selectedRef = useRef(selectedId);
  const highlightRef = useRef(highlightIds);
  const [hover, setHover] = useState<{ label: string; x: number; y: number } | null>(null);

  useEffect(() => {
    onSelectRef.current = onSelect;
    selectedRef.current = selectedId;
    highlightRef.current = highlightIds;
  });

  const { top = 0, right = 0, bottom = 0, left = 0 } = insets ?? {};
  useEffect(() => {
    insetsRef.current = { top, right, bottom, left };
    apiRef.current?.refit();
  }, [top, right, bottom, left]);

  useEffect(() => {
    const wrap = wrapRef.current;
    const overlay = overlayRef.current;
    if (!wrap || !overlay) return;

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    } catch {
      const msg = document.createElement("p");
      msg.className = "absolute inset-0 flex items-center justify-center text-[12.5px]";
      msg.style.color = "var(--ink-faint)";
      msg.textContent = "This view needs WebGL, which this browser has turned off.";
      overlay.replaceChildren(msg);
      return;
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    wrap.prepend(renderer.domElement);
    renderer.domElement.style.display = "block";

    const scene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(50, 1, 0.01, 100);
    const startDir = flat ? new THREE.Vector3(0, 0, 1) : START.clone().normalize();
    camera.position.copy(START);
    const controls = new OrbitControls(camera, renderer.domElement);
    const still = flat || window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    controls.enableDamping = true;
    controls.autoRotate = !still;
    controls.autoRotateSpeed = 0.6;
    controls.minDistance = 0.4;
    controls.maxDistance = 8;
    // once the user orbits or zooms, resizes stop refitting their view
    let touched = false;
    controls.addEventListener("start", () => {
      controls.autoRotate = false;
      touched = true;
    });

    // fit into a unit sphere around the centroid
    const n = points.length;
    const c = points.reduce((a, p) => [a[0] + p.x / n, a[1] + p.y / n, a[2] + p.z / n], [0, 0, 0]);
    const radius = Math.max(1e-9, ...points.map((p) => Math.hypot(p.x - c[0], p.y - c[1], p.z - c[2])));
    const pos = points.map((p) => new THREE.Vector3((p.x - c[0]) / radius, (p.y - c[1]) / radius, (p.z - c[2]) / radius));
    const index = new Map(points.map((p, i) => [p.id, i]));

    const baseSize = n > 400 ? 0.016 : links.length || labels ? 0.045 : 0.035;
    const geo = n > 400 ? new THREE.SphereGeometry(1, 10, 8) : new THREE.SphereGeometry(1, 32, 20);
    // flat fill, plus a slightly larger back-face shell that shows only at the edge: a hairline ring
    const mesh = new THREE.InstancedMesh(geo, new THREE.MeshBasicMaterial(), Math.max(n, 1));
    const ring = new THREE.InstancedMesh(geo, new THREE.MeshBasicMaterial({ side: THREE.BackSide }), Math.max(n, 1));
    mesh.count = ring.count = n;
    const RING = 1.09;
    const m4 = new THREE.Matrix4();
    const place = (i: number, scale: number) => {
      const s = baseSize * (points[i].size ?? 1) * scale;
      m4.makeScale(s, s, s).setPosition(pos[i]);
      mesh.setMatrixAt(i, m4);
      m4.makeScale(s * RING, s * RING, s * RING).setPosition(pos[i]);
      ring.setMatrixAt(i, m4);
    };
    points.forEach((_, i) => place(i, 1));
    scene.add(mesh, ring);
    // line materials tinted from --ink-faint, re-tinted on theme change
    const faintLines: THREE.LineBasicMaterial[] = [];

    // all links in --ink-faint, plus the selected node's links in --ink on top
    const LINK_OPACITY = 0.4;
    const pairs: [number, number][] = [];
    for (const l of links) {
      const a = index.get(l.source);
      const b = index.get(l.target);
      if (a !== undefined && b !== undefined) pairs.push([a, b]);
    }
    const segments = (ps: [number, number][]) =>
      new THREE.Float32BufferAttribute(
        ps.flatMap(([a, b]) => [pos[a].x, pos[a].y, pos[a].z, pos[b].x, pos[b].y, pos[b].z]),
        3,
      );
    const linkMat = new THREE.LineBasicMaterial({ transparent: true, opacity: LINK_OPACITY });
    const edgeMat = new THREE.LineBasicMaterial({ transparent: true, opacity: 0 });
    const edgeGeo = new THREE.BufferGeometry();
    if (pairs.length) {
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", segments(pairs));
      faintLines.push(linkMat);
      scene.add(new THREE.LineSegments(g, linkMat), new THREE.LineSegments(edgeGeo, edgeMat));
    }

    // axis lines + labels at their positive ends
    const axisEnds: THREE.Vector3[] = [];
    if (axes) {
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute([-1.15, 0, 0, 1.15, 0, 0, 0, -1.15, 0, 0, 1.15, 0, 0, 0, -1.15, 0, 0, 1.15], 3));
      // theme border colours are rgba; three ignores alpha, so opacity is set here
      const axisMat = new THREE.LineBasicMaterial({ transparent: true, opacity: 0.6 });
      faintLines.push(axisMat);
      scene.add(new THREE.LineSegments(g, axisMat));
      const grid = new THREE.GridHelper(2.3, 10);
      const gm = grid.material as THREE.LineBasicMaterial;
      gm.vertexColors = false;
      gm.transparent = true;
      gm.opacity = 0.12;
      faintLines.push(gm);
      grid.position.y = -1.15;
      scene.add(grid);
      axisEnds.push(new THREE.Vector3(1.22, 0, 0), new THREE.Vector3(0, 1.22, 0), new THREE.Vector3(0, 0, 1.22));
    }

    const applyTheme = () => {
      const cache = new Map<string, THREE.Color>();
      // ring = the kind colour pulled toward --ink, so it reads in both themes
      const ink = cssColor("var(--ink)", cache);
      const tmp = new THREE.Color();
      points.forEach((p, i) => {
        const fill = cssColor(p.color, cache);
        mesh.setColorAt(i, fill);
        ring.setColorAt(i, tmp.copy(fill).lerp(ink, 0.45));
      });
      if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
      if (ring.instanceColor) ring.instanceColor.needsUpdate = true;
      edgeMat.color.copy(ink);
      const faint = cssColor("var(--ink-faint)", cache);
      faintLines.forEach((m) => {
        m.color.copy(faint);
        m.needsUpdate = true;
      });
    };
    applyTheme();
    const themeWatch = new MutationObserver(applyTheme);
    themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    // HTML labels, repositioned every frame (no React re-render)
    overlay.replaceChildren();
    const labelled = labels && n <= MAX_LABELS ? points.map((_, i) => i) : [];
    const tag = (text: string, faint = false) => {
      const el = document.createElement("span");
      el.textContent = text;
      el.className = "scene3d-label";
      if (faint) el.style.color = "var(--ink-faint)";
      overlay.appendChild(el);
      return el;
    };
    // names are not code: point labels in Geist sans (axis ids stay mono)
    const sans = (el: HTMLSpanElement) => {
      el.style.font = "500 11.5px var(--font-sans), system-ui, sans-serif";
      return el;
    };
    const pointTags = labelled.map((i) => sans(tag(points[i].label.length > 22 ? `${points[i].label.slice(0, 21)}…` : points[i].label)));
    const axisTags = (axes ?? []).map((a) => tag(a, true));
    const selectedTag = sans(tag(""));
    selectedTag.style.color = "var(--ink)";
    // label widths for collision, re-measured once webfonts land
    let widths: number[] = [];
    const measure = () => (widths = pointTags.map((el) => el.offsetWidth || el.textContent!.length * 6.5));
    document.fonts?.ready.then(() => requestAnimationFrame(measure));
    // Figma-style selection frame around the selected node
    const selFrame = document.createElement("div");
    selFrame.className = "frame-selected";
    Object.assign(selFrame.style, { position: "absolute", left: "0", top: "0", display: "none" });
    overlay.appendChild(selFrame);
    // numbered frames for a multi-selection, one per highlighted point (pooled)
    const marks: { box: HTMLDivElement; num: HTMLSpanElement }[] = [];
    const mark = (k: number) => {
      if (!marks[k]) {
        const box = document.createElement("div");
        Object.assign(box.style, { position: "absolute", left: "0", top: "0", display: "none", border: "1px solid var(--accent)", borderRadius: "50%" });
        const num = document.createElement("span");
        Object.assign(num.style, {
          position: "absolute",
          right: "-6px",
          top: "-8px",
          minWidth: "14px",
          height: "14px",
          padding: "0 3px",
          borderRadius: "7px",
          background: "var(--accent)",
          color: "var(--on-accent)",
          font: "600 9.5px/14px var(--font-mono), monospace",
          textAlign: "center",
        });
        box.appendChild(num);
        overlay.appendChild(box);
        marks[k] = { box, num };
      }
      return marks[k];
    };
    // pointer pick ring: positioned per frame, animated by WAAPI on its own scale
    const pickRing = document.createElement("div");
    Object.assign(pickRing.style, { position: "absolute", left: "0", top: "0", display: "none" });
    const pickInner = document.createElement("div");
    Object.assign(pickInner.style, { width: "100%", height: "100%", borderRadius: "50%", border: "1px solid var(--accent)" });
    pickRing.appendChild(pickInner);
    overlay.appendChild(pickRing);
    let pickIdx: number | undefined;
    // hover reticle: four corner brackets, each placed per frame
    const corners = (["top left", "top right", "bottom left", "bottom right"] as const).map((pos) => {
      const el = document.createElement("span");
      const [v, h] = pos.split(" ");
      const edge = "1px solid var(--ink-dim)";
      Object.assign(el.style, {
        position: "absolute",
        left: "0",
        top: "0",
        width: "6px",
        height: "6px",
        display: "none",
        [`border${v[0].toUpperCase()}${v.slice(1)}`]: edge,
        [`border${h[0].toUpperCase()}${h.slice(1)}`]: edge,
      });
      overlay.appendChild(el);
      return el;
    });
    let hoverIdx: number | undefined;
    const reticle: { idx?: number; from?: Box; shown?: Box; start: number; mode: "in" | "move" } = { start: 0, mode: "in" };

    const size = { w: 1, h: 1 };
    let baseDistance = START.length();
    // Fit the camera to the node bounds inside the area the floating UI leaves
    // free. The view offset shifts the projection centre into that area.
    const fit = () => {
      const ins = insetsRef.current;
      const { w, h } = size;
      camera.setViewOffset(w, h, (ins.right - ins.left) / 2, (ins.bottom - ins.top) / 2, w, h);
      if (!flat) {
        camera.updateProjectionMatrix();
        return;
      }
      const f = h / 2 / Math.tan((camera.fov * Math.PI) / 360);
      // room for a label (below and either side of a node) at the edges; the
      // 2.4 floor keeps a lone node at its normal size instead of filling the view
      const hw = Math.max(40, (w - ins.left - ins.right) / 2 - 64);
      const hh = Math.max(40, (h - ins.top - ins.bottom) / 2 - 34);
      const box = new THREE.Box3().setFromPoints(pos);
      const centre = box.getCenter(new THREE.Vector3());
      const half = box.getSize(new THREE.Vector3()).multiplyScalar(0.5);
      const d = THREE.MathUtils.clamp(half.z + Math.max((f * half.x) / hw, (f * half.y) / hh, 2.4), controls.minDistance, controls.maxDistance);
      controls.target.copy(centre);
      camera.position.copy(centre).addScaledVector(startDir, d);
      camera.lookAt(centre);
      baseDistance = d;
      camera.updateProjectionMatrix();
    };
    const resize = () => {
      size.w = wrap.clientWidth;
      size.h = wrap.clientHeight;
      renderer.setSize(size.w, size.h);
      camera.aspect = size.w / Math.max(size.h, 1);
      if (touched) {
        const ins = insetsRef.current;
        camera.setViewOffset(size.w, size.h, (ins.right - ins.left) / 2, (ins.bottom - ins.top) / 2, size.w, size.h);
        camera.updateProjectionMatrix();
      } else fit();
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(wrap);

    const v = new THREE.Vector3();
    const project = (el: HTMLElement, p: THREE.Vector3, dy = 0) => {
      v.copy(p).project(camera);
      const hidden = v.z > 1;
      el.style.display = hidden ? "none" : "block";
      el.style.transform = `translate(-50%, 0) translate(${((v.x + 1) / 2) * size.w}px, ${((1 - v.y) / 2) * size.h + dy}px)`;
    };

    const focal = () => size.h / 2 / Math.tan((camera.fov * Math.PI) / 360);
    let lastZoom = "";
    // zoom scrub: log-distance the drag asks for (may run past the limits) and the release settle
    let scrubLog: number | null = null;
    let settle: { from: number; to: number; start: number } | null = null;
    const LIMITS = { min: controls.minDistance, max: controls.maxDistance };
    const setDistance = (d: number) => {
      const offset = camera.position.clone().sub(controls.target);
      camera.position.copy(controls.target).add(offset.setLength(d));
    };

    apiRef.current = {
      zoom: (factor) => {
        controls.autoRotate = false;
        touched = true;
        const offset = camera.position.clone().sub(controls.target);
        const d = THREE.MathUtils.clamp(offset.length() * factor, controls.minDistance, controls.maxDistance);
        camera.position.copy(controls.target).add(offset.setLength(d));
      },
      reset: () => {
        touched = false;
        controls.target.set(0, 0, 0);
        camera.position.copy(START);
        controls.autoRotate = !still;
        fit();
      },
      refit: () => {
        if (touched) resize();
        else fit();
      },
      // dx px of drag; rate is per px in log space (Shift coarse, Alt fine)
      scrub: (dx, rate) => {
        controls.autoRotate = false;
        touched = true;
        settle = null;
        scrubLog ??= Math.log(camera.position.distanceTo(controls.target));
        // dragging right zooms in, so distance shrinks
        scrubLog -= dx * rate;
        const lo = Math.log(LIMITS.min);
        const hi = Math.log(LIMITS.max);
        // rubber band: past a limit the view gives way less the further you drag (at most ~16%)
        const band = (over: number) => 0.15 * (1 - Math.exp(-over / 0.15));
        // past ~0.6 the band is saturated; capping here lets a reversed drag respond at once
        scrubLog = THREE.MathUtils.clamp(scrubLog, lo - 0.6, hi + 0.6);
        const eff = scrubLog < lo ? lo - band(lo - scrubLog) : scrubLog > hi ? hi + band(scrubLog - hi) : scrubLog;
        controls.minDistance = Math.min(LIMITS.min, Math.exp(eff));
        controls.maxDistance = Math.max(LIMITS.max, Math.exp(eff));
        setDistance(Math.exp(eff));
      },
      scrubEnd: () => {
        scrubLog = null;
        const d = camera.position.distanceTo(controls.target);
        const to = THREE.MathUtils.clamp(d, LIMITS.min, LIMITS.max);
        if (to === d || prefersReducedMotion()) {
          setDistance(to);
          Object.assign(controls, { minDistance: LIMITS.min, maxDistance: LIMITS.max });
        } else settle = { from: d, to, start: performance.now() };
      },
    };

    // Greedy label collision: the selected label first, then bigger nodes;
    // a label that would overlap one already placed steps down a row (up to 3).
    const order = labelled
      .map((_, k) => k)
      .sort((a, b) => (points[labelled[b]].size ?? 1) - (points[labelled[a]].size ?? 1));
    const placed: Box[] = [];
    // where each labelled node's label sits this frame, for the hover reticle
    const labelBox: (Box | undefined)[] = [];
    const placeLabels = (sel: number | undefined, below: (i: number, scale?: number) => number) => {
      if (!labelled.length) return;
      if (widths.length !== labelled.length) measure();
      placed.length = 0;
      const ks = sel === undefined ? order : [labelled.indexOf(sel), ...order.filter((k) => labelled[k] !== sel)].filter((k) => k >= 0);
      for (const k of ks) {
        const i = labelled[k];
        const el = pointTags[k];
        v.copy(pos[i]).project(camera);
        if (v.z > 1) {
          el.style.display = "none";
          labelBox[i] = undefined;
          continue;
        }
        const x = ((v.x + 1) / 2) * size.w;
        let y = ((1 - v.y) / 2) * size.h + (i === sel ? below(i, SELECTED_SCALE) + 6 : below(i));
        const half = widths[k] / 2 + 3;
        for (let step = 0; step < 3; step++) {
          const hit = placed.some((r) => x - half < r.x1 && x + half > r.x0 && y < r.y1 && y + LABEL_H > r.y0);
          if (!hit) break;
          y += LABEL_H;
        }
        placed.push({ x0: x - half, x1: x + half, y0: y, y1: y + LABEL_H });
        labelBox[i] = { x0: x - half, x1: x + half, y0: y, y1: y + LABEL_H };
        el.style.display = "block";
        el.style.transform = `translate(-50%, 0) translate(${x}px, ${y}px)`;
      }
    };

    // projected disc of node i: centre and radius in px
    const disc = (i: number, scale = 1) => {
      v.copy(pos[i]).project(camera);
      const r = (baseSize * (points[i].size ?? 1) * scale * focal()) / camera.position.distanceTo(pos[i]);
      return { x: ((v.x + 1) / 2) * size.w, y: ((1 - v.y) / 2) * size.h, r, hidden: v.z > 1 };
    };
    // 0 = no selection emphasis, 1 = incident edges bright and the rest dim
    const edge = { t: 0, from: 0, to: 0, start: 0 };

    let lastSelected: number | undefined;
    let raf = 0;
    const frame = () => {
      raf = requestAnimationFrame(frame);
      const now = performance.now();
      if (settle) {
        const t = Math.min(1, (now - settle.start) / SETTLE_MS);
        setDistance(settle.from + (settle.to - settle.from) * easeOut(t));
        if (t === 1) {
          settle = null;
          Object.assign(controls, { minDistance: LIMITS.min, maxDistance: LIMITS.max });
        }
      }
      controls.update();
      const sel = selectedRef.current ? index.get(selectedRef.current) : undefined;
      if (sel !== lastSelected) {
        const incident = sel === undefined ? [] : pairs.filter(([a, b]) => a === sel || b === sel);
        if (incident.length) edgeGeo.setAttribute("position", segments(incident));
        const to = incident.length ? 1 : 0;
        if (to !== edge.to) Object.assign(edge, { from: edge.t, to, start: pointerDriven() ? now : -Infinity });
        if (lastSelected !== undefined) {
          place(lastSelected, 1);
          const k = labelled.indexOf(lastSelected);
          if (k >= 0) pointTags[k].style.color = "";
        }
        if (sel !== undefined) {
          place(sel, SELECTED_SCALE);
          const k = labelled.indexOf(sel);
          if (k >= 0) pointTags[k].style.color = "var(--ink)";
        }
        mesh.instanceMatrix.needsUpdate = true;
        ring.instanceMatrix.needsUpdate = true;
        lastSelected = sel;
      }
      edge.t = edge.from + (edge.to - edge.from) * easeOut(Math.min(1, (now - edge.start) / EDGE_MS));
      linkMat.opacity = LINK_OPACITY * (1 - 0.6 * edge.t);
      edgeMat.opacity = 0.85 * edge.t;
      renderer.render(scene, camera);
      // labels sit just under each disc's projected edge
      const f = focal();
      const below = (i: number, scale = 1) => (baseSize * (points[i].size ?? 1) * scale * f) / camera.position.distanceTo(pos[i]) + 4;
      placeLabels(sel, below);
      axisEnds.forEach((p, k) => project(axisTags[k], p));
      if (sel !== undefined && !labelled.includes(sel)) {
        selectedTag.textContent = points[sel].label;
        project(selectedTag, pos[sel], below(sel, SELECTED_SCALE) + 6);
      } else selectedTag.style.display = "none";
      if (sel !== undefined) {
        v.copy(pos[sel]).project(camera);
        if (v.z > 1) selFrame.style.display = "none";
        else {
          const r = (baseSize * (points[sel].size ?? 1) * SELECTED_SCALE * focal()) / camera.position.distanceTo(pos[sel]);
          const box = Math.round(2 * r + 10);
          selFrame.style.display = "block";
          selFrame.style.width = selFrame.style.height = `${box}px`;
          selFrame.style.transform = `translate(${((v.x + 1) / 2) * size.w - box / 2}px, ${((1 - v.y) / 2) * size.h - box / 2}px)`;
        }
      } else selFrame.style.display = "none";
      const hl = highlightRef.current ?? [];
      for (let k = 0; k < Math.max(hl.length, marks.length); k++) {
        const at = k < hl.length ? index.get(hl[k]) : undefined;
        if (at === undefined) {
          if (marks[k]) marks[k].box.style.display = "none";
          continue;
        }
        const d = disc(at, at === sel ? SELECTED_SCALE : 1);
        const m = mark(k);
        if (d.hidden) {
          m.box.style.display = "none";
          continue;
        }
        const box = Math.round(2 * d.r + 8);
        m.box.style.display = "block";
        m.box.style.width = m.box.style.height = `${box}px`;
        m.box.style.transform = `translate(${d.x - box / 2}px, ${d.y - box / 2}px)`;
        if (m.num.textContent !== String(k + 1)) m.num.textContent = String(k + 1);
      }
      if (pickIdx !== undefined) {
        const p = disc(pickIdx, pickIdx === sel ? SELECTED_SCALE : 1);
        const d = Math.round(2 * p.r + 4);
        pickRing.style.display = p.hidden ? "none" : "block";
        pickRing.style.width = pickRing.style.height = `${d}px`;
        pickRing.style.transform = `translate(${p.x - d / 2}px, ${p.y - d / 2}px)`;
      }
      // reticle: brackets around the hovered node's label (or its disc when unlabelled)
      let target: Box | undefined;
      if (hoverIdx !== undefined) {
        const lb = labelBox[hoverIdx];
        const p = disc(hoverIdx, hoverIdx === sel ? SELECTED_SCALE : 1);
        if (lb && !p.hidden) target = { x0: lb.x0 - 3, x1: lb.x1 + 3, y0: lb.y0 - 3, y1: lb.y1 + 2 };
        else if (!p.hidden) target = { x0: p.x - p.r - 4, x1: p.x + p.r + 4, y0: p.y - p.r - 4, y1: p.y + p.r + 4 };
      }
      if (!target) {
        reticle.idx = reticle.shown = undefined;
        corners.forEach((c) => (c.style.display = "none"));
      } else {
        if (hoverIdx !== reticle.idx) {
          const still = prefersReducedMotion();
          Object.assign(reticle, { idx: hoverIdx, from: reticle.shown, start: still ? -Infinity : now, mode: reticle.shown ? "move" : "in" });
        }
        let box = target;
        let opacity = 1;
        if (reticle.mode === "move" && reticle.from) {
          const k = easeInOut(Math.min(1, (now - reticle.start) / RETICLE_MOVE_MS));
          const f = reticle.from;
          box = { x0: f.x0 + (target.x0 - f.x0) * k, x1: f.x1 + (target.x1 - f.x1) * k, y0: f.y0 + (target.y0 - f.y0) * k, y1: f.y1 + (target.y1 - f.y1) * k };
        } else if (reticle.mode === "in") {
          const k = easeOut(Math.min(1, (now - reticle.start) / RETICLE_IN_MS));
          const s = 1.5 - 0.5 * k;
          const cx = (target.x0 + target.x1) / 2;
          const cy = (target.y0 + target.y1) / 2;
          box = { x0: cx + (target.x0 - cx) * s, x1: cx + (target.x1 - cx) * s, y0: cy + (target.y0 - cy) * s, y1: cy + (target.y1 - cy) * s };
          opacity = k;
        }
        reticle.shown = box;
        const at = [
          [box.x0, box.y0],
          [box.x1 - 6, box.y0],
          [box.x0, box.y1 - 6],
          [box.x1 - 6, box.y1 - 6],
        ];
        corners.forEach((c, k) => {
          c.style.display = "block";
          c.style.opacity = `${opacity}`;
          c.style.transform = `translate(${at[k][0]}px, ${at[k][1]}px)`;
        });
      }
      if (zoomTextRef.current) {
        const z = `${Math.round((baseDistance / camera.position.distanceTo(controls.target)) * 100)}%`;
        if (z !== lastZoom) {
          zoomTextRef.current.textContent = lastZoom = z;
          zoomTextRef.current.setAttribute("aria-valuenow", z.slice(0, -1));
          zoomTextRef.current.setAttribute("aria-valuetext", z);
        }
      }
    };
    frame();

    // hover + click via raycasting
    const ray = new THREE.Raycaster();
    const ndc = new THREE.Vector2();
    const pick = (e: PointerEvent) => {
      const r = renderer.domElement.getBoundingClientRect();
      ndc.set(((e.clientX - r.left) / r.width) * 2 - 1, -((e.clientY - r.top) / r.height) * 2 + 1);
      ray.setFromCamera(ndc, camera);
      return ray.intersectObject(mesh)[0]?.instanceId;
    };
    let down: { x: number; y: number } | null = null;
    const onDown = (e: PointerEvent) => (down = { x: e.clientX, y: e.clientY });
    const fine = window.matchMedia("(hover: hover) and (pointer: fine)");
    const onUp = (e: PointerEvent) => {
      if (down && Math.hypot(e.clientX - down.x, e.clientY - down.y) < CLICK_SLOP) {
        const i = pick(e);
        if (i !== undefined) {
          onSelectRef.current?.(points[i].id);
          if (!prefersReducedMotion()) {
            pickIdx = i;
            pickInner.getAnimations().forEach((a) => a.cancel());
            pickInner
              .animate([{ transform: "scale(1)", opacity: 1 }, { transform: "scale(2.2)", opacity: 0 }], {
                duration: PICK_MS,
                easing: cssVar("--ease-out") || "ease-out",
                fill: "forwards",
              })
              .finished.then(() => {
                pickIdx = undefined;
                pickRing.style.display = "none";
              })
              .catch(() => {}); // cancelled by a newer pick
          }
        }
      }
      down = null;
    };
    const onMove = (e: PointerEvent) => {
      if (e.buttons) return;
      const i = pick(e);
      const r = wrap.getBoundingClientRect();
      hoverIdx = fine.matches && e.pointerType === "mouse" ? i : undefined;
      // a drawn, untruncated label under the reticle already names the node
      const named = i !== undefined && hoverIdx === i && labelled.length > 0 && points[i].label.length <= 22;
      setHover(i === undefined || named ? null : { label: points[i].label, x: e.clientX - r.left, y: e.clientY - r.top });
      renderer.domElement.style.cursor = i === undefined ? "grab" : "pointer";
    };
    const canvas = renderer.domElement;
    canvas.addEventListener("pointerdown", onDown);
    canvas.addEventListener("pointerup", onUp);
    canvas.addEventListener("pointermove", onMove);
    canvas.addEventListener("pointerleave", () => {
      hoverIdx = undefined;
      setHover(null);
    });

    return () => {
      cancelAnimationFrame(raf);
      apiRef.current = null;
      pickInner.getAnimations().forEach((an) => an.cancel());
      edgeGeo.dispose();
      edgeMat.dispose();
      themeWatch.disconnect();
      ro.disconnect();
      controls.dispose();
      renderer.dispose();
      scene.traverse((o) => {
        if (o instanceof THREE.Mesh || o instanceof THREE.LineSegments) {
          o.geometry.dispose(); // shared geometry: a second dispose is a no-op
          (o.material as THREE.Material).dispose();
        }
      });
      canvas.remove();
      overlay.replaceChildren();
    };
  }, [points, links, axes, labels, flat]);

  return (
    <div className="relative w-full h-full overflow-hidden">
      <div ref={wrapRef} className="absolute inset-0" role="img" aria-label={ariaLabel}>
        <div ref={overlayRef} className="absolute inset-0 pointer-events-none" aria-hidden />
      </div>
      {hover && (
        <div
          className="panel absolute pointer-events-none px-2 py-1 text-[12px] max-w-xs"
          style={{ left: hover.x + 12, top: hover.y + 12, borderRadius: 6, color: "var(--ink)", boxShadow: "var(--shadow-pop)" }}
        >
          {hover.label}
        </div>
      )}
      {zoomControls && (
        <TooltipGroup>
          <div className="panel absolute right-3 bottom-3 hidden md:flex items-center gap-0.5 p-1" role="group" aria-label="Zoom">
            <Tooltip label="Zoom out">
              <button type="button" className="btn btn-ghost btn-sm btn-icon w-[26px]" aria-label="Zoom out" onClick={() => apiRef.current?.zoom(1.25)}>
                <Minus size={14} strokeWidth={1.75} />
              </button>
            </Tooltip>
            {/* Figma-style scrub: drag left/right (Shift coarse, Alt fine); arrows step like the buttons */}
            <span
              ref={zoomTextRef}
              role="slider"
              tabIndex={0}
              aria-label="Zoom level, drag to change"
              aria-valuenow={100}
              aria-valuetext="100%"
              className="font-mono text-[12px] w-12 h-[26px] leading-[26px] text-center rounded-[5px] select-none touch-none cursor-ew-resize hover:bg-[var(--surface-raised)]"
              style={{ color: "var(--ink-dim)" }}
              onPointerDown={(e) => {
                if (e.button !== 0) return;
                e.currentTarget.setPointerCapture(e.pointerId);
                scrubRef.current = { id: e.pointerId, x: e.clientX };
              }}
              onPointerMove={(e) => {
                const s = scrubRef.current;
                if (!s || s.id !== e.pointerId) return;
                apiRef.current?.scrub(e.clientX - s.x, e.shiftKey ? 0.02 : e.altKey ? 0.0015 : 0.006);
                s.x = e.clientX;
              }}
              onPointerUp={() => {
                scrubRef.current = null;
                apiRef.current?.scrubEnd();
              }}
              onPointerCancel={() => {
                scrubRef.current = null;
                apiRef.current?.scrubEnd();
              }}
              onKeyDown={(e) => {
                const f = e.key === "ArrowRight" || e.key === "ArrowUp" ? 0.8 : e.key === "ArrowLeft" || e.key === "ArrowDown" ? 1.25 : 0;
                if (!f) return;
                e.preventDefault();
                apiRef.current?.zoom(f);
              }}
            >
              100%
            </span>
            <Tooltip label="Zoom in">
              <button type="button" className="btn btn-ghost btn-sm btn-icon w-[26px]" aria-label="Zoom in" onClick={() => apiRef.current?.zoom(0.8)}>
                <Plus size={14} strokeWidth={1.75} />
              </button>
            </Tooltip>
            <span className="w-px h-4 mx-0.5" style={{ background: "var(--border)" }} aria-hidden />
            <Tooltip label="Reset view">
              <button type="button" className="btn btn-ghost btn-sm btn-icon w-[26px]" aria-label="Reset view" onClick={() => apiRef.current?.reset()}>
                <Maximize size={14} strokeWidth={1.75} />
              </button>
            </Tooltip>
          </div>
        </TooltipGroup>
      )}
      {/* keyboard / screen-reader access to every point */}
      {onSelect && (
        <ul className="sr-only" aria-label={`${ariaLabel} items`}>
          {points.slice(0, 500).map((p) => (
            <li key={p.id}>
              <button type="button" onClick={() => onSelect(p.id)}>
                {p.label}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
