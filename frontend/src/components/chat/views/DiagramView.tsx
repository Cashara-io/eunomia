"use client";
import { useId, useState, type CSSProperties } from "react";
import Link from "next/link";
import {
  CubeIcon,
  UsersIcon,
  PackageIcon,
  ShoppingCartIcon,
  ReceiptIcon,
  MapPinIcon,
  WarehouseIcon,
  TruckIcon,
} from "@phosphor-icons/react";
import { useKindColor } from "@/lib/kindColors";
import { words, withUnit } from "./bits";
import type { DiagramView as Diagram } from "./diagram-schema";
import styles from "./diagram.module.css";
const families: Record<string, string> = {
  customer: "person",
  employee: "person",
  supplier: "person",
  product: "repository",
  stock_item: "repository",
  order: "company",
  purchase_order: "company",
  sale: "company",
  invoice: "file",
  payment: "file",
  contract: "symbol",
  delivery: "organisation",
  shipment: "organisation",
  delivery_route: "organisation",
  warehouse: "location",
};
const icons: Record<string, typeof CubeIcon> = {
  customer: UsersIcon,
  employee: UsersIcon,
  person: UsersIcon,
  supplier: UsersIcon,
  product: PackageIcon,
  stock_item: PackageIcon,
  order: ShoppingCartIcon,
  purchase_order: ShoppingCartIcon,
  invoice: ReceiptIcon,
  payment: ReceiptIcon,
  location: MapPinIcon,
  warehouse: WarehouseIcon,
  shipment: TruckIcon,
  delivery: TruckIcon,
};
export default function DiagramView({ view }: { view: Diagram }) {
  const d = view.diagram,
    [table, setTable] = useState(false),
    id = useId().replaceAll(":", ""),
    color = useKindColor();
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
          <h3 id={`${id}-title`}>{d.title}</h3>
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
            <svg
              viewBox={`0 0 320 ${d.height}`}
              role="img"
              aria-label={d.title}
              aria-describedby={`${id}-text`}
            >
              <defs>
                <marker
                  id={`${id}-arrow`}
                  markerWidth="7"
                  markerHeight="7"
                  refX="6"
                  refY="3.5"
                  orient="auto"
                >
                  <path d="M0 0 7 3.5 0 7" fill="var(--ink-dim)" />
                </marker>
                <pattern
                  id={`${id}-grid`}
                  width="32"
                  height="20"
                  patternUnits="userSpaceOnUse"
                  patternTransform="skewY(-14)"
                >
                  <path
                    d="M32 0H0V20"
                    fill="none"
                    stroke="var(--border)"
                    strokeWidth="0.5"
                  />
                </pattern>
              </defs>
              <rect
                width="320"
                height={d.height}
                fill={`url(#${id}-grid)`}
                opacity="0.7"
              />
              {d.edges.map((e, index) => {
                const a = nodes.get(e.from),
                  b = nodes.get(e.to);
                if (!a || !b) return null;
                const same = a.y === b.y,
                  x1 = a.x,
                  x2 = b.x,
                  y1 = same
                    ? a.y - 38
                    : a.y + (a.y < b.y ? 57 + a.metrics.length * 18 : -43),
                  y2 = same
                    ? b.y - 38
                    : b.y + (a.y < b.y ? -45 : 59 + b.metrics.length * 18),
                  lane = 20 + (index % 3) * 8;
                const path = same
                  ? `M${x1} ${y1} C${x1} ${y1 - lane} ${x2} ${y2 - lane} ${x2} ${y2}`
                  : `M${x1} ${y1} C${x1} ${(y1 + y2) / 2} ${x2} ${(y1 + y2) / 2} ${x2} ${y2}`;
                return (
                  <g key={e.id} className={styles.edge}>
                    <path
                      d={path}
                      fill="none"
                      markerEnd={`url(#${id}-arrow)`}
                    />
                    <text
                      x={(x1 + x2) / 2 + (same ? 0 : 28)}
                      y={same ? y1 - lane * 0.75 - 5 : (y1 + y2) / 2 - 22}
                      textAnchor="middle"
                    >
                      {words(e.label)}
                      {e.count !== undefined ? ` · ${e.count}` : ""}
                    </text>
                  </g>
                );
              })}
              {d.nodes.map((n) => {
                const Icon = icons[n.kind] ?? CubeIcon,
                  family = families[n.kind] ?? n.kind,
                  tint =
                    family === "company"
                      ? "var(--vault-company)"
                      : color(family);
                return (
                  <g
                    key={n.id}
                    transform={`translate(${n.x},${n.y})`}
                    data-focus={n.focus || n.emphasis}
                    className={styles.node}
                    style={{ "--diagram-hue": tint } as CSSProperties}
                  >
                    <title>
                      {n.name} ({words(n.kind)})
                    </title>
                    <path
                      className={styles.plinth}
                      d="M-62 -8 0 -33 62 -8 0 17Z"
                    />
                    <path
                      className={styles.side}
                      d="M-62 -8 0 17 62 -8 62 2 0 28 -62 2Z"
                    />
                    <Icon
                      x="-10"
                      y="-31"
                      width="20"
                      height="20"
                      weight="duotone"
                      aria-hidden
                    />
                    <rect
                      className={styles.plate}
                      x="-70"
                      y="3"
                      width="140"
                      height={51 + n.metrics.length * 18}
                      rx="5"
                    />
                    <foreignObject
                      x="-65"
                      y="7"
                      width="130"
                      height={44 + n.metrics.length * 18}
                    >
                      <div className={styles.label}>
                        <strong>{n.name}</strong>
                        <span>
                          {words(n.kind)}
                          {n.count !== undefined && ` · ${n.count}`}
                        </span>
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
                );
              })}
            </svg>
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
