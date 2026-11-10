"use client";
import { useEffect, useRef, useId, useState } from "react";
import Link from "next/link";
import { useLandscapeTint } from "../../twin/landscape-shared";
import LandscapeSolid from "../../twin/LandscapeSolid";
import ConnectionKindIcon from "../../twin/ConnectionKindIcon";
import { humanLabel, plural } from "../../twin/overview-format";
import { diagramGeometry } from "./diagram-geometry";
import { words, withUnit } from "./bits";
import type { DiagramView as Diagram } from "./diagram-schema";
import shared from "../../twin/overview.module.css";
import styles from "./diagram.module.css";
export default function DiagramView({
  view,
  title,
}: {
  view: Diagram;
  title?: string;
}) {
  const d = view.diagram,
    [table, setTable] = useState(false),
    id = useId().replaceAll(":", "");
  const heading = title ?? d.title;
  const nodes = new Map(d.nodes.map((n) => [n.id, n]));
  const focus = d.nodes.find((n) => n.focus) ?? d.nodes[0];
  const href =
    focus &&
    (d.diagram_type === "kinds"
      ? `/ontology?space=${encodeURIComponent(d.vault)}`
      : `/ontology/objects/${encodeURIComponent(focus.id)}?space=${encodeURIComponent(d.vault)}`);
  return (
    <figure
      className={styles.card}
      data-diagram={d.diagram_type}
      aria-labelledby={`${id}-title`}
    >
      <figcaption className={styles.header}>
        <div>
          <span className={styles.eyebrow}>
            {d.diagram_type === "kinds" ? "Ontology" : "Connected objects"}
          </span>
          <h3 id={`${id}-title`}>{heading}</h3>
        </div>
        <button
          type="button"
          className={styles.toggle}
          aria-pressed={table}
          aria-controls={`${id}-content`}
          onClick={() => setTable(!table)}
        >
          {table ? "Diagram" : "Table"}
        </button>
      </figcaption>
      <div className={styles.summary}>
        {d.total_partial && "At least "}
        {d.total_nodes} {d.diagram_type === "kinds" ? "kinds" : "objects"} ·{" "}
        {d.total_edges} links{d.partial && " · Partial view"}
      </div>
      {d.notice && <p className={styles.notice}>{d.notice}</p>}
      <div id={`${id}-content`}>
        {table ? (
          <div className={styles.tableWrap}>
            <table>
              <caption>Objects and annotations</caption>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Kind</th>
                  <th>Values</th>
                </tr>
              </thead>
              <tbody>
                {d.nodes.map((n) => (
                  <tr key={n.id}>
                    <td>{n.name}</td>
                    <td>{words(n.kind)}</td>
                    <td>
                      {n.count !== undefined && `${n.count} objects`}
                      {n.metrics.map((m) => (
                        <div key={m.metric}>
                          {words(m.metric)}:{" "}
                          {m.value === null
                            ? "No value"
                            : withUnit(m.value, m.unit)}
                        </div>
                      ))}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <table>
              <caption>Directed links</caption>
              <thead>
                <tr>
                  <th>From</th>
                  <th>Relation</th>
                  <th>To</th>
                  <th>Count</th>
                </tr>
              </thead>
              <tbody>
                {d.edges.map((e) => (
                  <tr key={e.id}>
                    <td>{nodes.get(e.from)?.name}</td>
                    <td>{words(e.label)}</td>
                    <td>{nodes.get(e.to)?.name}</td>
                    <td>{e.count ?? ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div className={styles.plane}>
            <p className={styles.scrollCue}>
              Follow the arrows · Scroll within the picture to explore ↔ ↕
            </p>
            {[false, true].map((phone) => (
              <DiagramPicture
                key={String(phone)}
                d={d}
                title={heading}
                id={`${id}-${phone}`}
                phone={phone}
              />
            ))}
          </div>
        )}
      </div>
      <div id={`${id}-text`} className="sr-only">
        {view.text}
      </div>
      {(d.groups.length > 0 || d.omitted_edges > 0) && (
        <p className={styles.notice}>
          {d.groups.map((g) => `${g.count} more ${words(g.kind)}`).join("; ")}
          {d.omitted_edges > 0 && ` · ${d.omitted_edges} more links`}. Counts
          cover the resolved selection
          {d.total_partial ? "; traversal also reached a resource limit" : ""}.
        </p>
      )}
      <footer className={styles.footer}>
        <span>Recorded links · Saved with this reply</span>
        {href && <Link href={href}>Open in Ontology ↗</Link>}
      </footer>
    </figure>
  );
}

function DiagramPicture({
  d,
  title,
  id,
  phone,
}: {
  d: Diagram["diagram"];
  title: string;
  id: string;
  phone: boolean;
}) {
  const g = diagramGeometry(d, phone),
    { tint } = useLandscapeTint();
  const viewport = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const frame = viewport.current;
    if (!frame || d.diagram_type !== "neighbourhood") return;
    const centre = () => {
      const focus = frame.querySelector('[data-focus="true"]');
      if (!focus || !frame.clientWidth) return;
      const box = focus.getBoundingClientRect(),
        bounds = frame.getBoundingClientRect();
      frame.scrollLeft +=
        box.x + box.width / 2 - bounds.x - frame.clientWidth / 2;
      frame.scrollTop +=
        box.y + box.height / 2 - bounds.y - frame.clientHeight / 2;
    };
    const observer = new ResizeObserver(centre);
    observer.observe(frame);
    return () => observer.disconnect();
  }, [d]);
  const maxWidth =
    phone && (d.diagram_type === "path" || d.diagram_type === "impact")
      ? 320
      : 670;
  const scale = Math.min(1, maxWidth / g.width);
  return (
    <div className={phone ? styles.phone : styles.desktop}>
      <div
        ref={viewport}
        className={styles.viewport}
        tabIndex={0}
        role="region"
        aria-label="Scrollable ontology picture"
      >
        <svg
          viewBox={`${g.left} ${g.top} ${g.width} ${g.height}`}
          style={{
            width: Math.max(g.width * scale, g.width * (phone ? 0.86 : 0.67)),
          }}
          role="img"
          aria-label={title}
          aria-describedby={`${id.replace(/-(true|false)$/, "")}-text`}
        >
          <defs>
            <marker
              id={`${id}-arrow`}
              markerWidth="8"
              markerHeight="8"
              refX="7"
              refY="4"
              orient="auto"
            >
              <path d="M0 0 8 4 0 8Z" fill="var(--ink)" />
            </marker>
            <pattern
              id={`${id}-grid`}
              width="42"
              height="42"
              patternTransform="matrix(1 -.05 .36 .62 0 0)"
              patternUnits="userSpaceOnUse"
            >
              <path
                d="M42 0H0V42"
                fill="none"
                stroke="var(--border-strong)"
                strokeWidth="0.5"
              />
            </pattern>
          </defs>
          <rect
            x={g.left}
            y={g.top}
            width={g.width}
            height={g.height}
            fill={`url(#${id}-grid)`}
            opacity=".3"
          />
          {!g.nodes.length && (
            <text x="20" y="50" className={styles.band}>
              No objects in this selection
            </text>
          )}
          {g.bands.map((b) => (
            <text key={b.text} x={b.x} y={b.y + 20} className={styles.band}>
              {b.text}
            </text>
          ))}
          {g.nodes.map((n) => (
            <g key={n.id} className={shared.form} style={tint(n.kind)}>
              <g transform={`translate(${n.x * 0.2},${n.y * 0.2}) scale(.8)`}>
                <LandscapeSolid x={n.x} y={n.y} height={n.height / 0.8} />
              </g>
            </g>
          ))}
          {g.edges.map((e) => (
            <g
              key={e.id}
              className={styles.edge}
              data-edge-from={e.from}
              data-edge-to={e.to}
            >
              <path d={e.path} fill="none" markerEnd={`url(#${id}-arrow)`} />
              <title>{e.label}</title>
            </g>
          ))}
          {g.nodes.map((n) => (
            <g
              key={n.id}
              data-node-box={n.id}
              className={`${shared.form} ${styles.node}`}
              style={tint(n.kind)}
              data-focus={n.focus || n.emphasis}
            >
              <title>
                {n.name} ({humanLabel(n.kind)})
              </title>
              <rect
                x={n.box.x}
                y={n.box.y}
                width={n.box.w}
                height={n.box.h}
                fill="transparent"
                pointerEvents="none"
              />
              <rect
                className={styles.plate}
                x={n.x - 80}
                y={n.box.y + 6}
                width="160"
                height={n.plateHeight}
                rx="7"
              />
              <foreignObject
                x={n.x - 76}
                y={n.box.y + 9}
                width="152"
                height={n.plateHeight - 4}
              >
                <div className={styles.label}>
                  <span className={styles.kind}>
                    <ConnectionKindIcon kind={n.kind} />
                    {d.diagram_type === "kinds"
                      ? `${n.count ?? 0} ${plural(humanLabel(n.kind).toLowerCase(), n.count)}`
                      : humanLabel(n.kind)}
                  </span>
                  {d.diagram_type !== "kinds" && <strong>{n.name}</strong>}
                  {n.metrics.map((m) => (
                    <span key={m.metric}>
                      {words(m.metric)}:{" "}
                      {m.value === null
                        ? "No value"
                        : withUnit(m.value, m.unit)}
                    </span>
                  ))}
                </div>
              </foreignObject>
            </g>
          ))}
          {g.edges.map((e) => (
            <g key={e.id} data-label-box={e.id} className={styles.verb}>
              <rect
                x={e.box.x}
                y={e.box.y}
                width={e.box.w}
                height={e.box.h}
                rx="8"
              />
              <foreignObject
                x={e.box.x + 4}
                y={e.box.y}
                width={e.box.w - 8}
                height={e.box.h}
              >
                <div>{e.label}</div>
              </foreignObject>
            </g>
          ))}
        </svg>
      </div>
    </div>
  );
}
