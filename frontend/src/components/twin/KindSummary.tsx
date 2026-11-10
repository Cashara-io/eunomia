"use client";
import {
  useEffect,
  useMemo,
  useId,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import Link from "next/link";
import { Bar } from "@visx/shape";
import { scaleLinear } from "@visx/scale";
import {
  ArrowRightIcon,
  GraphIcon,
  TableIcon,
  PackageIcon,
  UsersIcon,
  ReceiptIcon,
  ShoppingCartIcon,
  WarehouseIcon,
  MapPinIcon,
  TruckIcon,
  CubeIcon,
  XIcon,
  WarningCircleIcon,
} from "@phosphor-icons/react";
import type { KindItem, OntologyOut } from "@/lib/queries/ontology";
import { useKindColor } from "@/lib/kindColors";
import { twinHref } from "./model";
import { businessSummary } from "./overview-model";
import { humanLabel, plural } from "./overview-format";
import { linkKey, overviewLayout, routeLabels } from "./overview-layout";
import styles from "./overview.module.css";
import { useSetOverviewHighlight } from "./OverviewHighlight";

const icons: Record<string, typeof CubeIcon> = {
  customer: UsersIcon,
  employee: UsersIcon,
  person: UsersIcon,
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
  delivery_route: TruckIcon,
};
export type LandscapePin = {
  kind: string;
  label: string;
  href: string;
  type: "proposal" | "stale";
};
type Highlight = { type: "kind"; key: string } | { type: "link"; key: string };

export default function KindSummary({
  data,
  space,
  pins = [],
}: {
  data: OntologyOut;
  space: string;
  pins?: LandscapePin[];
}) {
  const { kinds } = useMemo(() => businessSummary(data), [data]);
  const links = useMemo(
    () =>
      data.links
        .filter(
          (l) =>
            l.status !== "rejected" &&
            l.count > 0 &&
            kinds.some((k) => k.kind === l.from_kind) &&
            kinds.some((k) => k.kind === l.to_kind),
        )
        .sort(
          (a, b) => b.count - a.count || linkKey(a).localeCompare(linkKey(b)),
        ),
    [data.links, kinds],
  );
  const layout = useMemo(() => overviewLayout(kinds, links), [kinds, links]);
  const unlinkedNodes = layout.nodes.filter((n) => !n.linked);
  const unlinkedArea = unlinkedNodes.length
    ? {
        left: Math.min(...unlinkedNodes.map((n) => n.x)) - 94,
        right: Math.max(...unlinkedNodes.map((n) => n.x)) + 94,
        top: Math.min(...unlinkedNodes.map((n) => n.y)) - 34,
        bottom: Math.max(...unlinkedNodes.map((n) => n.y)) + 32,
      }
    : null;
  const [table, setTable] = useState(false);
  const [hovered, setHovered] = useState<Highlight | null>(null);
  const [focused, setFocused] = useState<Highlight | null>(null);
  const [selected, setSelected] = useState<Highlight | null>(null);
  const active = focused ?? hovered ?? selected;
  const nodeRefs = useRef(new Map<string, HTMLButtonElement>());
  const color = useKindColor();
  const id = useId().replaceAll(":", "");
  const focusKind =
    selected?.type === "kind"
      ? kinds.find((k) => k.kind === selected.key)
      : undefined;
  const focusLink =
    selected?.type === "link"
      ? links.find((l) => linkKey(l) === selected.key)
      : undefined;
  const activeLink =
    active?.type === "link"
      ? links.find((l) => linkKey(l) === active.key)
      : undefined;
  const neighbours = new Set(
    active?.type === "kind"
      ? [
          active.key,
          ...links.flatMap((l) =>
            l.from_kind === active.key
              ? [l.to_kind]
              : l.to_kind === active.key
                ? [l.from_kind]
                : [],
          ),
        ]
      : activeLink
        ? [activeLink.from_kind, activeLink.to_kind]
        : [],
  );
  const relevantLink = (key: string, from: string, to: string) =>
    !active ||
    (active.type === "link"
      ? active.key === key
      : from === active.key || to === active.key);
  const setKinds = useSetOverviewHighlight();
  const highlighted =
    active?.type === "kind"
      ? active.key
      : activeLink
        ? `${activeLink.from_kind}\n${activeLink.to_kind}`
        : "";
  useEffect(() => {
    setKinds(highlighted ? highlighted.split("\n") : []);
  }, [highlighted, setKinds]);
  const objectScale = scaleLinear({
    domain: [0, Math.max(1, ...kinds.map((k) => k.entities))],
    range: [0, 100],
  });
  const linkScale = scaleLinear({
    domain: [0, Math.max(1, ...links.map((l) => l.count))],
    range: [0, 100],
  });
  function choose(item: Highlight) {
    setSelected(
      selected?.type === item.type && selected.key === item.key ? null : item,
    );
    setHovered(null);
    setFocused(null);
  }
  function keyNode(event: KeyboardEvent<HTMLButtonElement>, kind: string) {
    if (
      ![
        "ArrowRight",
        "ArrowLeft",
        "ArrowDown",
        "ArrowUp",
        "Home",
        "End",
      ].includes(event.key)
    )
      return;
    event.preventDefault();
    const index = layout.nodes.findIndex((n) => n.kind.kind === kind);
    const step =
      event.key === "ArrowDown"
        ? 4
        : event.key === "ArrowUp"
          ? -4
          : event.key === "ArrowLeft"
            ? -1
            : 1;
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? layout.nodes.length - 1
          : (((index + step) % layout.nodes.length) + layout.nodes.length) %
            layout.nodes.length;
    const name = layout.nodes[next]?.kind.kind;
    if (name) nodeRefs.current.get(name)?.focus();
  }
  function nodeName(kind: KindItem) {
    const related = links.filter(
      (l) => l.from_kind === kind.kind || l.to_kind === kind.kind,
    );
    return `${plural(humanLabel(kind.kind))}, ${kind.entities.toLocaleString("en-AU")} ${plural("object", kind.entities)}. ${related.length ? related.map((l) => `${humanLabel(l.from_kind)} ${l.label.replaceAll("_", " ")} ${humanLabel(l.to_kind)}`).join(". ") : "Not linked yet"}`;
  }
  const detailLinks = focusKind
    ? links.filter(
        (l) => l.from_kind === focusKind.kind || l.to_kind === focusKind.kind,
      )
    : focusLink
      ? [focusLink]
      : [];
  return (
    <section
      className={styles.map}
      aria-labelledby="kind-map-title"
      onMouseMove={() => {
        if (focused) setFocused(null);
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          setSelected(null);
          setHovered(null);
          setFocused(null);
        }
      }}
    >
      <div className={styles.sectionHead}>
        <div>
          <h2 id="kind-map-title">How it fits together</h2>
          <p className={styles.caption}>
            A landscape of {kinds.length} {plural("kind", kinds.length)}. Height
            shows object count. Routes show recorded relationships.
          </p>
        </div>
        <button
          className="btn btn-ghost btn-sm"
          aria-pressed={table}
          onClick={() => setTable(!table)}
        >
          {table ? (
            <GraphIcon aria-hidden size={16} />
          ) : (
            <TableIcon aria-hidden size={16} />
          )}{" "}
          {table ? "Show landscape" : "Show table"}
        </button>
      </div>
      {pins.length > 0 && (
        <div className={styles.mapNotices}>
          {[
            pins.find((p) => p.type === "proposal"),
            pins.find((p) => p.type === "stale"),
          ]
            .filter((p) => p !== undefined)
            .map((pin) => (
              <Link key={pin.href} href={pin.href}>
                <WarningCircleIcon size={14} weight="fill" aria-hidden />
                {pin.label}
                <ArrowRightIcon size={12} aria-hidden />
              </Link>
            ))}
        </div>
      )}
      {table ? (
        <div
          className={styles.tableRegion}
          role="region"
          aria-label="Kind connections table"
          tabIndex={0}
        >
          <table className="data-table w-full">
            <caption>The same kinds as the landscape</caption>
            <thead>
              <tr>
                <th>Kind</th>
                <th>Objects</th>
                <th>Links in and out</th>
              </tr>
            </thead>
            <tbody>
              {layout.nodes.map(({ kind }) => (
                <tr key={kind.kind}>
                  <th scope="row">
                    <button
                      className="link"
                      onClick={() => choose({ type: "kind", key: kind.kind })}
                    >
                      {plural(humanLabel(kind.kind))}
                    </button>
                  </th>
                  <td className="font-mono">
                    {kind.entities.toLocaleString("en-AU")}
                  </td>
                  <td>
                    {links.filter(
                      (l) =>
                        l.from_kind === kind.kind || l.to_kind === kind.kind,
                    ).length || "Not linked yet"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <table className="data-table w-full">
            <caption>The same connections as the diagram</caption>
            <thead>
              <tr>
                <th>From</th>
                <th>Relationship</th>
                <th>To</th>
                <th>Records</th>
              </tr>
            </thead>
            <tbody>
              {links.map((l) => (
                <tr key={linkKey(l)}>
                  <td>{humanLabel(l.from_kind)}</td>
                  <td>
                    <button
                      className="link"
                      onClick={() => choose({ type: "link", key: linkKey(l) })}
                    >
                      {humanLabel(l.label)}
                    </button>
                  </td>
                  <td>{humanLabel(l.to_kind)}</td>
                  <td className="font-mono">
                    {l.count.toLocaleString("en-AU")}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div
          className={styles.landscape}
          data-testid="kind-landscape"
          style={
            { "--plane-ratio": `1200 / ${layout.height}` } as CSSProperties
          }
          onMouseLeave={() => setHovered(null)}
        >
          <svg
            className={styles.plane}
            viewBox={`0 0 1200 ${layout.height}`}
            preserveAspectRatio="none"
            aria-hidden="true"
          >
            <defs>
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
                  strokeWidth=".65"
                />
              </pattern>
              <marker
                id={`${id}-arrow`}
                viewBox="0 0 8 8"
                refX="7"
                refY="4"
                markerWidth="5"
                markerHeight="5"
                orient="auto-start-reverse"
              >
                <path
                  d="M1 1 7 4 1 7"
                  fill="none"
                  stroke="context-stroke"
                  strokeWidth="1.5"
                />
              </marker>
            </defs>
            <path
              d={`M22 148 1110 82 1180 ${layout.height - 80} 90 ${layout.height - 14}Z`}
              fill={`url(#${id}-grid)`}
              stroke="var(--border-strong)"
              strokeWidth="1"
            />
            {unlinkedArea && (
              <g>
                <path
                  d={`M${unlinkedArea.left} ${unlinkedArea.top}L${unlinkedArea.right} ${unlinkedArea.top - 18}V${unlinkedArea.bottom}L${unlinkedArea.left} ${unlinkedArea.bottom + 18}Z`}
                  fill="none"
                  stroke="var(--border-field)"
                  strokeDasharray="4 5"
                />
                <text
                  x={unlinkedArea.left}
                  y={unlinkedArea.bottom + 43}
                  fill="var(--ink-dim)"
                  fontSize="14"
                >
                  Not linked yet
                </text>
              </g>
            )}
            {layout.edges.map(({ link, path, key }, i) => (
              <path
                key={key}
                className={styles.route}
                data-active={
                  !!active && relevantLink(key, link.from_kind, link.to_kind)
                }
                data-dim={
                  !!active && !relevantLink(key, link.from_kind, link.to_kind)
                }
                d={path}
                fill="none"
                stroke="var(--ink-dim)"
                strokeWidth={0.7 + linkScale(link.count) / 65}
                markerEnd={`url(#${id}-arrow)`}
                style={{ opacity: active ? undefined : i < 3 ? 0.65 : 0.045 }}
              />
            ))}
            {layout.nodes.map(({ kind, x, y, height, linked }) => (
              <g
                key={kind.kind}
                className={styles.form}
                data-dim={!!active && !neighbours.has(kind.kind)}
                data-unlinked={!linked}
              >
                <ellipse
                  cx={x + 8}
                  cy={y + 10}
                  rx="70"
                  ry="24"
                  fill="var(--ink)"
                  opacity=".045"
                />
                <path
                  d={`M${x - 62} ${y - 9}l88 -14 38 24 -88 15Z`}
                  fill="var(--surface-raised)"
                  stroke="var(--border-field)"
                />
                <path
                  d={`M${x - 62} ${y - height - 9}l88 -14v${height}l-88 14Z`}
                  fill="var(--surface-raised)"
                  stroke="var(--border-field)"
                />
                <path
                  d={`M${x + 26} ${y - height - 23}l38 24v${height}l-38 -24Z`}
                  fill="var(--surface-hover)"
                  stroke="var(--border-field)"
                />
                <path
                  d={`M${x - 62} ${y - height - 9}l88 -14 38 24 -88 15Z`}
                  fill="var(--surface)"
                  stroke="var(--ink-dim)"
                />
                {[0.25, 0.5, 0.75].map((t) => (
                  <path
                    key={t}
                    d={`M${x - 62} ${y - height * t - 9}l88 -14 38 24`}
                    fill="none"
                    stroke="var(--border-field)"
                    strokeWidth=".6"
                  />
                ))}
                <path
                  d={`M${x} ${y - height}v-35`}
                  stroke="var(--ink-dim)"
                  strokeWidth="1"
                />
                <ellipse
                  cx={x}
                  cy={y - height}
                  rx="5"
                  ry="2"
                  fill={color(kind.kind)}
                />
              </g>
            ))}
          </svg>
          {layout.nodes.map(({ kind, x, y, height, linked }) => {
            const Icon = icons[kind.kind] ?? CubeIcon;
            const item: Highlight = { type: "kind", key: kind.kind };
            return (
              <div
                key={kind.kind}
                className={styles.landmark}
                data-dim={!!active && !neighbours.has(kind.kind)}
                data-linked={linked}
                style={{
                  left: `${x / 12}%`,
                  top: `${((y - height - 92) / layout.height) * 100}%`,
                }}
              >
                {!linked && layout.unlinked[0]?.kind === kind.kind && (
                  <span className={styles.unlinkedLabel}>Not linked yet</span>
                )}
                <button
                  ref={(el) => {
                    if (el) nodeRefs.current.set(kind.kind, el);
                    else nodeRefs.current.delete(kind.kind);
                  }}
                  data-kind={kind.kind}
                  className={styles.landmarkButton}
                  aria-label={nodeName(kind)}
                  aria-pressed={
                    selected?.type === "kind" && selected.key === kind.kind
                  }
                  onMouseEnter={() => setHovered(item)}
                  onFocus={() => {
                    setHovered(null);
                    setFocused(item);
                  }}
                  onBlur={() => setFocused(null)}
                  onClick={() => choose(item)}
                  onKeyDown={(e) => keyNode(e, kind.kind)}
                >
                  <span className={styles.kindTitle}>
                    {plural(humanLabel(kind.kind))}
                  </span>
                  <span className={styles.nodeCount}>
                    {kind.entities.toLocaleString("en-AU")}{" "}
                    {plural("object", kind.entities)}
                  </span>
                  <span className={styles.kindPin}>
                    <Icon
                      size={20}
                      weight="duotone"
                      style={
                        { "--duotone-color": color(kind.kind) } as CSSProperties
                      }
                      aria-hidden
                    />
                  </span>
                </button>
                {pins.filter((p) => p.kind === kind.kind).length > 0 && (
                  <div className={styles.attentionPins}>
                    {pins
                      .filter((p) => p.kind === kind.kind)
                      .map((pin) => (
                        <Link
                          key={pin.href}
                          href={pin.href}
                          className={styles.attentionPin}
                          aria-label={pin.label}
                          title={pin.label}
                        >
                          <WarningCircleIcon
                            size={16}
                            weight="fill"
                            aria-hidden
                          />
                        </Link>
                      ))}
                  </div>
                )}
                <ul className={styles.flatLinks}>
                  {links
                    .filter((l) => l.from_kind === kind.kind)
                    .map((l) => (
                      <li key={linkKey(l)}>
                        <button
                          onClick={() =>
                            choose({ type: "link", key: linkKey(l) })
                          }
                          onFocus={() => {
                            setHovered(null);
                            setFocused({ type: "link", key: linkKey(l) });
                          }}
                          onBlur={() => setFocused(null)}
                        >
                          {humanLabel(l.label)}{" "}
                          <ArrowRightIcon size={12} aria-hidden />{" "}
                          {plural(humanLabel(l.to_kind))}{" "}
                          <span className={styles.nodeCount}>
                            {l.count.toLocaleString("en-AU")}
                          </span>
                        </button>
                      </li>
                    ))}
                  {!linked && <li>No recorded links for this kind.</li>}
                </ul>
              </div>
            );
          })}
          <div className={styles.routeLabels}>
            {routeLabels(
              layout,
              layout.edges
                .filter(({ link, key }, i) =>
                  active
                    ? relevantLink(key, link.from_kind, link.to_kind)
                    : i < 3,
                )
                .slice(0, 3),
            ).map(({ link, key, x, y }) => (
              <button
                key={key}
                className={styles.routeLabel}
                style={{
                  left: `${x / 12}%`,
                  top: `${(y / layout.height) * 100}%`,
                }}
                onMouseEnter={() => setHovered({ type: "link", key })}
                onFocus={() => {
                  setHovered(null);
                  setFocused({ type: "link", key });
                }}
                onBlur={() => setFocused(null)}
                onClick={() => choose({ type: "link", key })}
                aria-pressed={selected?.type === "link" && selected.key === key}
              >
                {humanLabel(link.label)}{" "}
                <ArrowRightIcon aria-hidden size={12} />
              </button>
            ))}
          </div>
        </div>
      )}
      <div className={styles.mapFoot}>
        <span>
          {kinds.length} {plural("kind", kinds.length)} · {links.length}{" "}
          recorded link types
          {!active && links.length > 3
            ? " · Strongest 3 routes labelled, all routes in the linked panel"
            : ""}
        </span>
        <Link className="link" href={twinHref("/ontology/schema", space)}>
          All structure <ArrowRightIcon size={14} aria-hidden />
        </Link>
      </div>
      <div className={styles.inspector} aria-live="polite">
        {focusKind || focusLink ? (
          <>
            <div className={styles.sectionHead}>
              <h3>
                {focusKind
                  ? plural(humanLabel(focusKind.kind))
                  : `${humanLabel(focusLink?.from_kind ?? "")} ${humanLabel(focusLink?.label ?? "").toLowerCase()} ${humanLabel(focusLink?.to_kind ?? "")}`}
              </h3>
              <button
                className="btn btn-ghost btn-icon btn-sm"
                aria-label="Clear selection"
                onClick={() => {
                  setSelected(null);
                  setHovered(null);
                  setFocused(null);
                }}
              >
                <XIcon size={16} />
              </button>
            </div>
            <p>
              {focusKind
                ? `${focusKind.entities.toLocaleString("en-AU")} ${plural("object", focusKind.entities)}. ${focusKind.samples.length ? `Examples: ${focusKind.samples.slice(0, 3).join(", ")}.` : "No named examples returned."} Definition: ${focusKind.status === "observed" ? "used, not reviewed" : focusKind.status}.`
                : `${focusLink?.count.toLocaleString("en-AU")} recorded links. ${
                    focusLink?.samples.length
                      ? focusLink.samples
                          .slice(0, 2)
                          .map((s) => `${s.from} to ${s.to}`)
                          .join("; ")
                      : "No named examples returned."
                  }`}
            </p>
            <ul className={styles.detailLinks}>
              {detailLinks.map((l) => (
                <li key={linkKey(l)}>
                  {focusKind && l.to_kind === focusKind.kind ? "In: " : "Out: "}
                  {humanLabel(l.from_kind)} {humanLabel(l.label).toLowerCase()}{" "}
                  {humanLabel(l.to_kind)}{" "}
                  <span className={styles.nodeCount}>
                    {l.count.toLocaleString("en-AU")}
                  </span>
                </li>
              ))}
            </ul>
            <Link
              className="link"
              href={twinHref("/ontology/objects", space, {
                kind: focusKind?.kind ?? focusLink?.from_kind ?? "",
              })}
            >
              Find{" "}
              {plural(
                humanLabel(focusKind?.kind ?? focusLink?.from_kind ?? ""),
              )}{" "}
              <ArrowRightIcon size={14} aria-hidden />
            </Link>
          </>
        ) : (
          <p className={styles.caption}>
            Explore a kind or a route. Select to keep its examples and
            connections open here.
          </p>
        )}
      </div>
      <div className={styles.linkedPanels}>
        <section aria-label="Objects per kind" className={styles.chartPanel}>
          <h3>
            Objects per kind{" "}
            <span className={styles.caption}>Same scale across all kinds</span>
          </h3>
          <div className={styles.kindBars}>
            {layout.nodes.map(({ kind }) => (
              <button
                key={kind.kind}
                className={styles.barRow}
                data-dim={!!active && !neighbours.has(kind.kind)}
                aria-label={`${plural(humanLabel(kind.kind))}: ${kind.entities.toLocaleString("en-AU")} objects`}
                onMouseEnter={() =>
                  setHovered({ type: "kind", key: kind.kind })
                }
                onMouseLeave={() => setHovered(null)}
                onFocus={() => {
                  setHovered(null);
                  setFocused({ type: "kind", key: kind.kind });
                }}
                onBlur={() => setFocused(null)}
                onClick={() => choose({ type: "kind", key: kind.kind })}
              >
                <span>{plural(humanLabel(kind.kind))}</span>
                <span className={styles.nodeCount}>
                  {kind.entities.toLocaleString("en-AU")}
                </span>
                <svg viewBox="0 0 100 5" preserveAspectRatio="none" aria-hidden>
                  <Bar
                    x={0}
                    y={0}
                    width={objectScale(kind.entities)}
                    height={5}
                    fill="var(--ink)"
                    rx={1}
                  />
                </svg>
              </button>
            ))}
          </div>
        </section>
        <section aria-label="Links per link type" className={styles.chartPanel}>
          <h3>
            Recorded links{" "}
            <span className={styles.caption}>
              Follow a route across the landscape
            </span>
          </h3>
          <div className={styles.linkBars}>
            {links.map((l) => (
              <button
                key={linkKey(l)}
                className={styles.barRow}
                data-dim={
                  !!active && !relevantLink(linkKey(l), l.from_kind, l.to_kind)
                }
                onMouseEnter={() =>
                  setHovered({ type: "link", key: linkKey(l) })
                }
                onMouseLeave={() => setHovered(null)}
                onFocus={() => {
                  setHovered(null);
                  setFocused({ type: "link", key: linkKey(l) });
                }}
                onBlur={() => setFocused(null)}
                onClick={() => choose({ type: "link", key: linkKey(l) })}
              >
                <span>
                  {humanLabel(l.from_kind)}{" "}
                  <b>{humanLabel(l.label).toLowerCase()}</b>{" "}
                  {humanLabel(l.to_kind)}
                </span>
                <span className={styles.nodeCount}>
                  {l.count.toLocaleString("en-AU")}
                </span>
                <svg viewBox="0 0 100 5" preserveAspectRatio="none" aria-hidden>
                  <Bar
                    width={linkScale(l.count)}
                    height={5}
                    fill="var(--ink)"
                    rx={1}
                  />
                </svg>
              </button>
            ))}
            {links.length === 0 && (
              <p className={styles.caption}>No recorded links yet.</p>
            )}
          </div>
        </section>
      </div>
    </section>
  );
}
