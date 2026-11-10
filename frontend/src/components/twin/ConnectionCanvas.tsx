import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import type { ConnectionNode } from "@/lib/queries/connections";
import {
  connectionsLayout,
  edgeSentence,
  type Layout,
  type Scene,
} from "./connections-model";
import { humanLabel, plural } from "./overview-format";
import { useLandscapeTint } from "./landscape-shared";
import LandscapeSolid from "./LandscapeSolid";
import shared from "./overview.module.css";
import styles from "./connections.module.css";

type Camera = { x: number; y: number; zoom: number };
export default function ConnectionCanvas({
  scene,
  focus,
  selected,
  active,
  select,
  hover,
}: {
  scene: Scene;
  focus: string;
  selected: string;
  active: string;
  select: (id: string) => void;
  hover: (id: string) => void;
}) {
  const [layout, setLayout] = useState<Layout | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [size, setSize] = useState({ width: 1000, height: 540 });
  const [camera, setCamera] = useState<Camera>({ x: -200, y: -80, zoom: 1 });
  const viewport = useRef<HTMLDivElement>(null);
  const plane = useRef<HTMLDivElement>(null);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const worker = useRef<Worker | null>(null);
  const id = useId().replaceAll(":", "");
  const { tint } = useLandscapeTint();
  useEffect(() => {
    const w = new Worker(new URL("./connections.worker.ts", import.meta.url));
    worker.current = w;
    w.onmessage = (e: MessageEvent<Layout>) => {
      setLayout(e.data);
      performance.mark("connections-layout-ready");
    };
    w.onerror = () => setLayout(connectionsLayout(scene, focus));
    w.postMessage({ scene, focus });
    return () => {
      w.terminate();
      worker.current = null;
    };
  }, [scene, focus]);
  useEffect(() => {
    const el = viewport.current;
    if (el) {
      setSize({ width: el.clientWidth, height: el.clientHeight });
      setCamera({
        x: el.clientWidth / 2 - 600,
        y: el.clientHeight / 2 - 370,
        zoom: 1,
      });
    }
  }, []);
  const shown = useMemo(
    () =>
      layout?.nodes.filter(
        (n) =>
          !n.group ||
          (layout.groups.find((g) => g.id === n.group)?.count ?? 0) === 1 ||
          expanded.has(n.group),
      ) ?? [],
    [layout, expanded],
  );
  const groups = layout?.groups.filter((g) => g.count > 1) ?? [];
  const anchors = new Map<string, { x: number; y: number; id: string }>();
  for (const n of layout?.nodes ?? []) {
    const group = groups.find((g) => g.id === n.group);
    anchors.set(
      n.id,
      group && !expanded.has(n.group) ? { ...group, id: group.id } : n,
    );
  }
  const nodeMap = new Map(scene.nodes.map((n) => [n.id, n]));
  const routes = new Map<
    string,
    {
      from: { x: number; y: number; id: string };
      to: { x: number; y: number; id: string };
      label: string;
      sentences: string[];
      active: boolean;
    }
  >();
  for (const edge of scene.edges) {
    const from = anchors.get(edge.from),
      to = anchors.get(edge.to);
    if (!from || !to || from.id === to.id) continue;
    const key = JSON.stringify([from.id, edge.label, to.id]);
    const route = routes.get(key) ?? {
      from,
      to,
      label: edge.label,
      sentences: [],
      active: false,
    };
    route.sentences.push(edgeSentence(edge, nodeMap));
    route.active ||= edge.from === active || edge.to === active;
    routes.set(key, route);
  }
  const entries = [...routes.entries()];
  function move(next: Camera) {
    setCamera(next);
  }
  function zoom(amount: number) {
    const width = viewport.current?.clientWidth ?? 800,
      height = viewport.current?.clientHeight ?? 550;
    const z = Math.min(1.8, Math.max(0.8, camera.zoom + amount));
    move({
      zoom: z,
      x: width / 2 - ((width / 2 - camera.x) * z) / camera.zoom,
      y: height / 2 - ((height / 2 - camera.y) * z) / camera.zoom,
    });
  }
  function fit() {
    const marks = [...shown, ...groups];
    if (!marks.length) return;
    const minX = Math.min(...marks.map((n) => n.x)) - 120,
      maxX = Math.max(...marks.map((n) => n.x)) + 120;
    const minY = Math.min(...marks.map((n) => n.y)) - 130,
      maxY = Math.max(...marks.map((n) => n.y)) + 60;
    const w = viewport.current?.clientWidth ?? 800,
      h = viewport.current?.clientHeight ?? 550;
    const z = Math.max(0.8, Math.min(1, w / (maxX - minX), h / (maxY - minY)));
    move({
      zoom: z,
      x: w / 2 - ((minX + maxX) / 2) * z,
      y: h / 2 - ((minY + maxY) / 2) * z,
    });
  }
  function keyNode(e: KeyboardEvent<HTMLButtonElement>, key: string) {
    const keys = [...shown.map((n) => n.id), ...groups.map((g) => g.id)];
    const index = keys.indexOf(key);
    const delta = ["ArrowRight", "ArrowDown"].includes(e.key)
      ? 1
      : ["ArrowLeft", "ArrowUp"].includes(e.key)
        ? -1
        : 0;
    if (delta) {
      e.preventDefault();
      const next = keys[(index + delta + keys.length) % keys.length];
      hover(next);
      const point =
        shown.find((n) => n.id === next) ?? groups.find((g) => g.id === next);
      if (point && viewport.current)
        move({
          ...camera,
          x: viewport.current.clientWidth / 2 - point.x * camera.zoom,
          y: viewport.current.clientHeight / 2 - point.y * camera.zoom,
        });
      requestAnimationFrame(() => buttons.current.get(next)?.focus());
    }
  }
  function nodeButton(
    node: ConnectionNode & { x: number; y: number },
    group?: Layout["groups"][number],
  ) {
    const key = group?.id ?? node.id;
    return (
      <div
        key={key}
        className={styles.mark}
        style={{ ...tint(node.kind), left: node.x, top: node.y }}
        data-active={active === node.id}
      >
        <button
          className={shared.landmarkButton}
          data-object={group ? undefined : node.id}
          data-group={group?.id}
          aria-expanded={group ? expanded.has(group.id) : undefined}
          aria-pressed={group ? undefined : selected === node.id}
          ref={(el) => {
            if (el) buttons.current.set(key, el);
            else buttons.current.delete(key);
          }}
          onKeyDown={(e) => keyNode(e, key)}
          onMouseEnter={() => hover(node.id)}
          onMouseLeave={() => hover("")}
          onFocus={() => {
            hover(node.id);
            if (!onScreen(node) && viewport.current)
              move({
                ...camera,
                x: viewport.current.clientWidth / 2 - node.x * camera.zoom,
                y: viewport.current.clientHeight / 2 - node.y * camera.zoom,
              });
          }}
          onBlur={() => hover("")}
          onClick={() =>
            group
              ? setExpanded((prev) => {
                  const next = new Set(prev);
                  if (next.has(group.id)) next.delete(group.id);
                  else next.add(group.id);
                  return next;
                })
              : select(node.id)
          }
        >
          <span className={styles.markKind}>
            {humanLabel(node.kind)}
            {group
              ? ` · ${group.hop} ${plural("hop", group.hop)}`
              : node.id === focus
                ? " · Focus"
                : ""}
          </span>
          <strong>
            {group
              ? `${group.count} ${plural(humanLabel(group.kind).toLowerCase(), group.count)}`
              : node.name}
          </strong>
          {group && (
            <span className={shared.nodeCount}>
              {expanded.has(group.id)
                ? "Close group"
                : `Open ${group.count} loaded objects`}
            </span>
          )}
        </button>
        {onScreen(node) && (
          <svg
            width="164"
            height="100"
            viewBox="-82 -78 164 100"
            aria-hidden
            className={shared.form}
            style={tint(node.kind)}
            data-selected={selected === node.id}
          >
            <LandscapeSolid
              x={0}
              y={0}
              height={node.id === focus ? 50 : group ? 36 : 24}
            />
          </svg>
        )}
      </div>
    );
  }
  const onScreen = (point: { x: number; y: number }) =>
    point.x * camera.zoom + camera.x > -200 &&
    point.x * camera.zoom + camera.x < size.width + 200 &&
    point.y * camera.zoom + camera.y > -100 &&
    point.y * camera.zoom + camera.y < size.height + 100;
  const inFrame = (point: { x: number; y: number }) =>
    point.x * camera.zoom + camera.x >= 110 &&
    point.x * camera.zoom + camera.x <= size.width - 110 &&
    point.y * camera.zoom + camera.y >= 80 &&
    point.y * camera.zoom + camera.y <= size.height - 80;
  const occupied = [...shown, ...groups].filter(onScreen).flatMap((n) => [
    { x: n.x - 110, y: n.y - 85, w: 220, h: 70 },
    { x: n.x - 85, y: n.y + 5, w: 170, h: 90 },
  ]);
  const labelPositions = new Map<string, { x: number; y: number }>();
  for (const [i, [key, route]] of entries.entries()) {
    if (
      (!route.active && i >= 4) ||
      !onScreen(route.from) ||
      !onScreen(route.to)
    )
      continue;
    for (const dy of [42, -42, 80, -80, 0]) {
      const x = (route.from.x + route.to.x) / 2,
        y = (route.from.y + route.to.y) / 2 + dy;
      const box = { x: x - 90, y: y - 14, w: 180, h: 28 };
      if (
        occupied.some(
          (r) =>
            box.x < r.x + r.w &&
            box.x + box.w > r.x &&
            box.y < r.y + r.h &&
            box.y + box.h > r.y,
        )
      )
        continue;
      occupied.push(box);
      labelPositions.set(key, { x, y });
      break;
    }
  }
  return (
    <div className={styles.canvasWrap}>
      <div
        className={styles.canvas}
        role="region"
        aria-label="Objects connections canvas"
        tabIndex={0}
        ref={viewport}
        onKeyDown={(e) => {
          if (e.key === "+" || e.key === "=") {
            e.preventDefault();
            zoom(0.1);
          }
          if (e.key === "-") {
            e.preventDefault();
            zoom(-0.1);
          }
        }}
      >
        <div
          className={styles.plane}
          ref={plane}
          data-layout-ms={layout?.ms}
          data-camera={`${camera.x},${camera.y},${camera.zoom}`}
          style={{
            transform: `translate(${camera.x}px,${camera.y}px) scale(${camera.zoom})`,
          }}
        >
          <svg
            className={styles.routes}
            viewBox="-2000 -2000 8000 12000"
            aria-hidden
          >
            <defs>
              <marker
                id={`${id}-arrow`}
                viewBox="0 0 8 8"
                refX="7"
                refY="4"
                markerWidth="6"
                markerHeight="6"
                orient="auto"
              >
                <path
                  d="M1 1 7 4 1 7"
                  fill="none"
                  stroke="context-stroke"
                  strokeWidth="1.5"
                />
              </marker>
            </defs>
            {entries.map(([key, r], i) => {
              if (!onScreen(r.from) || !onScreen(r.to)) return null;
              const visible = r.active || i < 4;
              const label = labelPositions.get(key);
              return (
                <g key={key} opacity={visible ? 1 : 0.12}>
                  <path
                    d={`M${r.from.x},${r.from.y} Q${(r.from.x + r.to.x) / 2},${(r.from.y + r.to.y) / 2 + 85} ${r.to.x},${r.to.y}`}
                    stroke="var(--ink-dim)"
                    strokeWidth={r.active ? 2.5 : 1.5}
                    fill="none"
                    markerEnd={`url(#${id}-arrow)`}
                  />
                  {visible && label && (
                    <g transform={`translate(${label.x},${label.y})`}>
                      <rect
                        data-route-label
                        x="-90"
                        y="-14"
                        width="180"
                        height="28"
                        rx="14"
                        fill="var(--surface)"
                        stroke="var(--border)"
                      />
                      <text
                        textAnchor="middle"
                        y="5"
                        fill="var(--ink)"
                        fontSize="13"
                      >
                        {humanLabel(r.label).toLowerCase()}
                      </text>
                    </g>
                  )}
                </g>
              );
            })}
          </svg>
          {shown
            .filter((n) => onScreen(n) || n.id === active || n.id === selected)
            .map((n) => nodeButton(n))}
          {groups
            .filter(onScreen)
            .map((g) => nodeButton({ ...g, name: "", facts: [] }, g))}
        </div>
        {!layout && (
          <p className={styles.canvasMessage}>Arranging this neighbourhood…</p>
        )}
      </div>
      <div className={styles.camera} aria-label="Canvas controls">
        <button className="btn btn-sm" onClick={fit}>
          Fit view
        </button>
        <button
          className="btn btn-sm"
          onClick={() => zoom(0.1)}
          aria-label="Zoom in"
        >
          +
        </button>
        <button
          className="btn btn-sm"
          onClick={() => zoom(-0.1)}
          aria-label="Zoom out"
        >
          −
        </button>
        {(
          [
            [-100, 0, "right"],
            [100, 0, "left"],
            [0, 100, "up"],
            [0, -100, "down"],
          ] as const
        ).map(([x, y, label]) => (
          <button
            className="btn btn-sm"
            key={label}
            aria-label={`Pan ${label}`}
            onClick={() =>
              move({ ...camera, x: camera.x + x, y: camera.y + y })
            }
          >
            {label === "right"
              ? "→"
              : label === "left"
                ? "←"
                : label === "up"
                  ? "↑"
                  : "↓"}
          </button>
        ))}
        <span>
          {Math.round(camera.zoom * 100)}% · {scene.nodes.length - shown.length}{" "}
          grouped · {shown.filter((n) => !inFrame(n)).length} beyond frame
        </span>
      </div>
      <p className={styles.help}>
        Arrows move between objects. Enter selects. Escape clears. + and − zoom.
        Pan to explore beyond the frame, or use the complete table.
      </p>
    </div>
  );
}
