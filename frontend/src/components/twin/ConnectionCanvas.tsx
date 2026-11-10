import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import {
  ArrowsOutSimpleIcon,
  PlusIcon,
  MinusIcon,
  ArrowUpIcon,
  ArrowDownIcon,
  ArrowLeftIcon,
  ArrowRightIcon,
  KeyboardIcon,
} from "@phosphor-icons/react";
import {
  connectionsLayout,
  edgeSentence,
  formCentre,
  formHeight,
  formPort,
  loadedDegrees,
  type FormAnchor,
  type Layout,
  type Scene,
} from "./connections-model";
import { humanLabel, plural } from "./overview-format";
import { useLandscapeTint } from "./landscape-shared";
import ConnectionKindIcon from "./ConnectionKindIcon";
import LandscapeSolid from "./LandscapeSolid";
import shared from "./overview.module.css";
import styles from "./connections.module.css";

type Camera = { x: number; y: number; zoom: number };
type Mark = FormAnchor & {
  kind: string;
  name: string;
  hop: number;
  group?: Layout["groups"][number];
};
const plateTop = (n: Mark) =>
  -n.height - (n.group ? 115 : n.scale > 1 ? 100 : 85);
export default function ConnectionCanvas({
  scene,
  focus,
  selected,
  active,
  activeKind,
  select,
  hover,
}: {
  scene: Scene;
  focus: string;
  selected: string;
  active: string;
  activeKind: string;
  select: (id: string) => void;
  hover: (id: string) => void;
}) {
  const [layout, setLayout] = useState<Layout | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [size, setSize] = useState({ width: 1000, height: 700 });
  const [manual, move] = useState<Camera | null>(null);
  const [help, setHelp] = useState(false);
  const [keyboardTarget, setKeyboardTarget] = useState("");
  const viewport = useRef<HTMLDivElement>(null);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const id = useId().replaceAll(":", "");
  const { tint } = useLandscapeTint();
  useEffect(() => {
    const w = new Worker(new URL("./connections.worker.ts", import.meta.url));
    w.onmessage = (e: MessageEvent<Layout>) => {
      setLayout(e.data);
      performance.mark("connections-layout-ready");
    };
    w.onerror = () => setLayout(connectionsLayout(scene, focus));
    w.postMessage({ scene, focus });
    return () => w.terminate();
  }, [scene, focus]);
  useEffect(() => {
    const el = viewport.current;
    if (!el) return;
    const observer = new ResizeObserver(() =>
      setSize({ width: el.clientWidth, height: el.clientHeight }),
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const degrees = useMemo(() => loadedDegrees(scene), [scene]);
  const marks = useMemo(() => {
    if (!layout) return [];
    const visible: Mark[] = layout.nodes
      .filter((n) => !n.group || expanded.has(n.group))
      .map((n) => ({
        ...n,
        group: undefined,
        height: formHeight(degrees.get(n.id) ?? 0),
        scale: n.id === focus ? 1.3 : 0.85,
      }));
    for (const g of layout.groups)
      visible.push({
        ...g,
        group: g,
        name: `${g.count} ${plural(humanLabel(g.kind).toLowerCase(), g.count)}`,
        height: formHeight(
          layout.nodes
            .filter((n) => n.group === g.id)
            .reduce((sum, n) => sum + (degrees.get(n.id) ?? 0), 0),
        ),
        scale: 1,
      });
    return visible;
  }, [layout, expanded, degrees, focus]);
  const fitCamera = useMemo(() => {
    if (!marks.length) return { x: 0, y: 0, zoom: 1 };
    const minX = Math.min(...marks.map((n) => n.x)) - 125,
      maxX = Math.max(...marks.map((n) => n.x)) + 125;
    const minY = Math.min(...marks.map((n) => n.y - n.height)) - 105,
      maxY = Math.max(...marks.map((n) => n.y)) + 85;
    const zoom = Math.min(
      1.05,
      (size.width - 36) / (maxX - minX),
      (size.height - 100) / (maxY - minY),
    );
    return {
      zoom,
      x: size.width / 2 - ((minX + maxX) / 2) * zoom,
      y: (size.height - 30) / 2 - ((minY + maxY) / 2) * zoom,
    };
  }, [marks, size]);
  const camera = manual ?? fitCamera;
  const onScreen = (n: { x: number; y: number }) =>
    n.x * camera.zoom + camera.x > -180 &&
    n.x * camera.zoom + camera.x < size.width + 180 &&
    n.y * camera.zoom + camera.y > -100 &&
    n.y * camera.zoom + camera.y < size.height + 160;
  const anchors = new Map<string, Mark>();
  for (const n of layout?.nodes ?? []) {
    const group = marks.find((g) => g.id === n.group && !expanded.has(n.group));
    const mark = group ?? marks.find((m) => m.id === n.id);
    if (mark) anchors.set(n.id, mark);
  }
  const nodeMap = new Map(scene.nodes.map((n) => [n.id, n]));
  const routes = new Map<
    string,
    {
      from: Mark;
      to: Mark;
      label: string;
      sentences: string[];
      active: boolean;
    }
  >();
  for (const edge of scene.edges) {
    const from = anchors.get(edge.from),
      to = anchors.get(edge.to);
    if (!from || !to || (from.id === to.id && edge.from !== edge.to)) continue;
    const key = JSON.stringify([from.id, edge.label, to.id]);
    const route = routes.get(key) ?? {
      from,
      to,
      label: edge.label,
      sentences: [],
      active: false,
    };
    route.sentences.push(edgeSentence(edge, nodeMap));
    route.active ||=
      edge.from === active ||
      edge.to === active ||
      from.id === active ||
      to.id === active ||
      (!!activeKind && (from.kind === activeKind || to.kind === activeKind));
    routes.set(key, route);
  }
  const occupied = marks.filter(onScreen).flatMap((n) => [
    { x: n.x - 102, y: n.y + plateTop(n) - 4, w: 204, h: 60 },
    {
      x: n.x - 85,
      y: n.y - n.height - 24,
      w: 170,
      h: n.height + 50,
    },
  ]);
  const intersects = (b: { x: number; y: number; w: number; h: number }) =>
    occupied.some(
      (r) =>
        b.x < r.x + r.w &&
        b.x + b.w > r.x &&
        b.y < r.y + r.h &&
        b.y + b.h > r.y,
    );
  const drawnRoutes = [...routes.entries()]
    .filter(([, r]) => onScreen(r.from) && onScreen(r.to))
    .map(([key, r]) => {
      const a = formCentre(r.from),
        b = formCentre(r.to);
      const self = r.from.id === r.to.id;
      let label = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      const width = Math.max(80, r.label.length * 8 + 28);
      let placed = false;
      for (const t of [0.52, 0.36, 0.68, 0.22, 0.8]) {
        for (const offset of [
          0, 36, -36, 70, -70, 110, -110, 160, -160, 220, -220,
        ]) {
          const len = Math.hypot(b.x - a.x, b.y - a.y) || 1;
          const p = self
            ? { x: a.x + 115, y: a.y - 25 }
            : {
                x: a.x + (b.x - a.x) * t - ((b.y - a.y) / len) * offset,
                y: a.y + (b.y - a.y) * t + ((b.x - a.x) / len) * offset,
              };
          const box = {
            x: p.x - width / 2 - 5,
            y: p.y - 19,
            w: width + 10,
            h: 38,
          };
          if (!intersects(box)) {
            occupied.push(box);
            label = p;
            placed = true;
            break;
          }
        }
        if (placed) break;
      }
      const control = {
        x: 2 * label.x - (a.x + b.x) / 2,
        y: 2 * label.y - (a.y + b.y) / 2,
      };
      const start = formPort(
        r.from,
        self ? { x: a.x + 80, y: a.y - 15 } : control,
      );
      const end = formPort(r.to, self ? { x: b.x + 80, y: b.y + 20 } : control);
      const d = `M${start.x},${start.y} Q${2 * label.x - (start.x + end.x) / 2},${2 * label.y - (start.y + end.y) / 2} ${end.x},${end.y}`;
      return { key, r, label, width, d, start, end };
    });
  function zoom(amount: number) {
    const z = Math.min(2, Math.max(0.2, camera.zoom + amount));
    move({
      zoom: z,
      x: size.width / 2 - ((size.width / 2 - camera.x) * z) / camera.zoom,
      y: size.height / 2 - ((size.height / 2 - camera.y) * z) / camera.zoom,
    });
  }
  function keyNode(e: KeyboardEvent<HTMLButtonElement>, key: string) {
    const delta = ["ArrowRight", "ArrowDown"].includes(e.key)
      ? 1
      : ["ArrowLeft", "ArrowUp"].includes(e.key)
        ? -1
        : 0;
    if (!delta) return;
    e.preventDefault();
    const n =
      marks[
        (marks.findIndex((n) => n.id === key) + delta + marks.length) %
          marks.length
      ];
    setKeyboardTarget(n.id);
    move({
      ...camera,
      x: size.width / 2 - n.x * camera.zoom,
      y: size.height / 2 - n.y * camera.zoom,
    });
    requestAnimationFrame(() => buttons.current.get(n.id)?.focus());
  }
  const highlighted = !!active || !!activeKind;
  const hops = [...new Set(Object.values(scene.hops).filter((n) => n > 0))];
  return (
    <div className={styles.canvasWrap}>
      <div
        className={styles.legend}
        title="Height uses the square root of the number of loaded links touching an object. Groups use the sum of their members’ link counts. Self-links count once."
      >
        <span className={styles.heightKey} aria-hidden>
          ▂ ▄ █
        </span>{" "}
        {scene.edges.length
          ? "Height = √ loaded links · stacked = grouped objects"
          : "No loaded links: equal heights"}{" "}
        <span>Focus has a larger footprint</span>
      </div>
      <div
        className={styles.canvas}
        data-compact={scene.nodes.length <= 6}
        role="region"
        aria-label="Objects connections canvas"
        tabIndex={0}
        ref={viewport}
        onKeyDown={(e) => {
          if (["+", "=", "-"].includes(e.key)) {
            e.preventDefault();
            zoom(e.key === "-" ? -0.1 : 0.1);
          }
          if (e.key === "?") {
            e.preventDefault();
            e.stopPropagation();
            setHelp((v) => !v);
          }
          if (e.key.toLowerCase() === "f") move(fitCamera);
        }}
      >
        <div
          className={styles.plane}
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
                markerWidth="7"
                markerHeight="7"
                orient="auto"
              >
                <path
                  d="M1 1 7 4 1 7"
                  fill="none"
                  stroke="context-stroke"
                  strokeWidth="1.4"
                />
              </marker>
            </defs>
            {hops.map((h) => (
              <g key={h} className={styles.contour} data-hop-ring={h}>
                {[0, 12, 24].map((offset) => (
                  <ellipse
                    key={offset}
                    cx="600"
                    cy="470"
                    rx={
                      (marks.length <= 6 ? 300 : 450) + (h - 1) * 350 + offset
                    }
                    ry={
                      (marks.length <= 6 ? 220 : 350) +
                      (h - 1) * 290 +
                      offset * 0.7
                    }
                  />
                ))}
                <text
                  x="600"
                  y={470 + (marks.length <= 6 ? 220 : 350) + (h - 1) * 290 + 48}
                  textAnchor="middle"
                >
                  {h} {plural("hop", h)}
                </text>
              </g>
            ))}
            <ellipse
              className={styles.focusRing}
              cx="600"
              cy="470"
              rx="124"
              ry="50"
            />
            <ellipse
              className={styles.focusRing}
              cx="600"
              cy="470"
              rx="133"
              ry="55"
            />
            {drawnRoutes.map(({ key, r, label, width, d, start, end }) => (
              <g
                key={key}
                className={styles.route}
                data-active={r.active}
                opacity={highlighted && !r.active ? 0.13 : 1}
              >
                <title>{r.sentences.join(". ")}</title>
                <path
                  data-edge-path
                  data-from={r.from.id}
                  data-to={r.to.id}
                  data-start={`${start.x},${start.y}`}
                  data-end={`${end.x},${end.y}`}
                  d={d}
                  markerEnd={`url(#${id}-arrow)`}
                />
                <g
                  transform={`translate(${label.x},${label.y})`}
                  data-edge-label
                >
                  <rect
                    x={-width / 2}
                    y="-15"
                    width={width}
                    height="30"
                    rx="15"
                  />
                  <text y="5" textAnchor="middle">
                    {r.label.replaceAll("_", " ")}
                  </text>
                </g>
              </g>
            ))}
          </svg>
          {marks
            .filter(
              (n) =>
                onScreen(n) ||
                n.id === active ||
                n.id === selected ||
                n.id === keyboardTarget,
            )
            .map((n) => {
              const lit =
                n.id === active || n.id === selected || n.kind === activeKind;
              const plateY = plateTop(n);
              return (
                <div
                  key={n.id}
                  className={styles.mark}
                  style={{ ...tint(n.kind), left: n.x, top: n.y }}
                  data-active={lit}
                  data-dimmed={!!activeKind && n.kind !== activeKind}
                  data-focus={n.id === focus}
                >
                  {n.id !== focus &&
                    marks.filter((m) => m.kind === n.kind && m.hop === n.hop)[
                      Math.floor(
                        marks.filter(
                          (m) => m.kind === n.kind && m.hop === n.hop,
                        ).length / 2,
                      )
                    ]?.id === n.id && (
                      <span className={styles.sector} style={{ top: 42 }}>
                        {plural(humanLabel(n.kind))}
                      </span>
                    )}
                  <svg
                    width="260"
                    height="250"
                    viewBox="-130 -200 260 250"
                    aria-hidden
                    className={`${shared.form} ${styles.objectForm}`}
                    data-form={n.id}
                    data-selected={selected === n.id}
                  >
                    {n.group &&
                      [22, 11].map((y) => (
                        <g key={y} transform={`translate(0,${y})`}>
                          <LandscapeSolid x={0} y={0} height={15} />
                        </g>
                      ))}
                    <g transform={`scale(${n.scale})`}>
                      <LandscapeSolid x={0} y={0} height={n.height / n.scale} />
                    </g>
                    <path
                      data-label-stalk
                      d={`M0,${plateY + (n.group ? 65 : 44)} L0,${-n.height - 4}`}
                      className={styles.stalk}
                    />
                    <circle
                      cx="0"
                      cy={-n.height - 4}
                      r="3"
                      fill="var(--kind-hue)"
                    />
                  </svg>
                  <button
                    className={styles.plate}
                    style={{ top: plateY }}
                    data-object={n.group ? undefined : n.id}
                    data-group={n.group?.id}
                    aria-expanded={n.group ? expanded.has(n.id) : undefined}
                    aria-pressed={n.group ? undefined : selected === n.id}
                    ref={(el) => {
                      if (el) buttons.current.set(n.id, el);
                      else buttons.current.delete(n.id);
                    }}
                    onKeyDown={(e) => keyNode(e, n.id)}
                    onMouseEnter={() => hover(n.id)}
                    onMouseLeave={() => hover("")}
                    onFocus={() => {
                      hover(n.id);
                      if (!onScreen(n))
                        move({
                          ...camera,
                          x: size.width / 2 - n.x * camera.zoom,
                          y: size.height / 2 - n.y * camera.zoom,
                        });
                    }}
                    onBlur={() => hover("")}
                    onClick={() => {
                      if (!n.group) select(n.id);
                      else {
                        move(camera);
                        setExpanded((prev) => {
                          const next = new Set(prev);
                          if (next.has(n.id)) next.delete(n.id);
                          else next.add(n.id);
                          return next;
                        });
                      }
                    }}
                  >
                    <span className={styles.markKind}>
                      <ConnectionKindIcon kind={n.kind} />
                      {humanLabel(n.kind)}
                      {n.id === focus ? " · Focus" : ""}
                    </span>
                    <strong>{n.name}</strong>
                    {n.group && (
                      <span className={styles.groupHint}>
                        {expanded.has(n.id)
                          ? "Close group"
                          : `Open ${n.group.count} loaded objects`}
                      </span>
                    )}
                  </button>
                </div>
              );
            })}
        </div>
        {!layout && (
          <p className={styles.canvasMessage}>Arranging this neighbourhood…</p>
        )}
        <div
          className={styles.camera}
          role="group"
          aria-label="Canvas controls"
        >
          <button
            onClick={() => move(fitCamera)}
            aria-label="Fit view"
            title="Fit view (F)"
          >
            <ArrowsOutSimpleIcon />
          </button>
          <button
            onClick={() => zoom(-0.1)}
            aria-label="Zoom out"
            title="Zoom out (-)"
          >
            <MinusIcon />
          </button>
          <output aria-label="Zoom">{Math.round(camera.zoom * 100)}%</output>
          <button
            onClick={() => zoom(0.1)}
            aria-label="Zoom in"
            title="Zoom in (+)"
          >
            <PlusIcon />
          </button>
          {[
            { x: -100, y: 0, name: "right", Icon: ArrowRightIcon },
            { x: 100, y: 0, name: "left", Icon: ArrowLeftIcon },
            { x: 0, y: 100, name: "up", Icon: ArrowUpIcon },
            { x: 0, y: -100, name: "down", Icon: ArrowDownIcon },
          ].map(({ x, y, name, Icon }) => (
            <button
              key={name}
              aria-label={`Pan ${name}`}
              title={`Pan ${name}`}
              onClick={() =>
                move({ ...camera, x: camera.x + x, y: camera.y + y })
              }
            >
              <Icon />
            </button>
          ))}
          <button
            aria-label="Keyboard shortcuts"
            title="Keyboard shortcuts (?)"
            aria-expanded={help}
            onClick={() => setHelp((v) => !v)}
          >
            <KeyboardIcon />
          </button>
        </div>
        {help && (
          <div className={styles.shortcuts}>
            <strong>Explore with your keyboard</strong>
            <p>
              Arrows move between objects. Enter selects. Escape clears
              selection. + and - zoom. F fits the view. ? toggles this hint.
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
