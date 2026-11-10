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
  ClockCountdownIcon,
  ChatCircleDotsIcon,
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

// Seven familiar palette families connect the plane, icons and linked panels.
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
export type LandscapePin = {
  kind: string;
  label: string;
  href: string;
  type: "proposal" | "stale";
  count?: number;
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
  const hue = (kind: string) =>
    families[kind] === "company"
      ? "var(--vault-company)"
      : color(families[kind] ?? kind);
  const tint = (kind: string) => ({ "--kind-hue": hue(kind) }) as CSSProperties;
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
  const labelledRoutes = routeLabels(
    layout,
    layout.edges
      .filter(({ link, key }, i) =>
        active ? relevantLink(key, link.from_kind, link.to_kind) : i < 3,
      )
      .slice(0, 3),
  );
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
      <div className={styles.heightLegend}>
        <span aria-hidden>▂ ▄ █</span> height = objects{" "}
        <span className={styles.caption}>
          Square-root scale · equal footprints
        </span>
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
              <linearGradient id={`${id}-fade`} x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="white" stopOpacity=".12" />
                <stop offset="75%" stopColor="white" stopOpacity=".8" />
              </linearGradient>
              <mask id={`${id}-ground`}>
                <rect
                  width="1200"
                  height={layout.height}
                  fill={`url(#${id}-fade)`}
                />
              </mask>
              <mask id={`${id}-labels`}>
                <rect width="1200" height={layout.height} fill="white" />
                {layout.nodes.map((n) => (
                  <rect
                    key={n.kind.kind}
                    x={n.x - 116}
                    y={n.y - n.height - 90}
                    width="232"
                    height="78"
                    rx="10"
                    fill="black"
                  />
                ))}
              </mask>
              <radialGradient id={`${id}-shadow`}>
                <stop stopColor="black" stopOpacity=".32" />
                <stop offset="1" stopColor="black" stopOpacity="0" />
              </radialGradient>
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
                  strokeWidth="1"
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
              d={`M22 120 1110 54 1180 ${layout.height - 80} 90 ${layout.height - 14}Z`}
              className={styles.ground}
              fill={`url(#${id}-grid)`}
              mask={`url(#${id}-ground)`}
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
            <g mask={`url(#${id}-labels)`}>
              {layout.edges.map(({ link, path, key }) => (
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
                  strokeWidth={1 + linkScale(link.count) / 110}
                  markerEnd={`url(#${id}-arrow)`}
                  style={tint(link.from_kind)}
                />
              ))}
              {active &&
                layout.edges
                  .filter(({ link, key }) =>
                    relevantLink(key, link.from_kind, link.to_kind),
                  )
                  .map(({ link, key, path }) => (
                    <path
                      key={`flow-${key}`}
                      className={styles.routeFlow}
                      data-active="true"
                      d={path}
                      fill="none"
                      style={tint(link.from_kind)}
                    />
                  ))}
            </g>
            {layout.nodes.map(({ kind, x, y, height, linked }) => (
              <g
                key={kind.kind}
                className={styles.form}
                data-dim={!!active && !neighbours.has(kind.kind)}
                data-unlinked={!linked}
                data-selected={
                  selected?.type === "kind" && selected.key === kind.kind
                }
                style={tint(kind.kind)}
              >
                <ellipse
                  cx={x + 8}
                  cy={y + 16}
                  rx="91"
                  ry="34"
                  fill={`url(#${id}-shadow)`}
                />
                <path
                  className={styles.baseRing}
                  d={`M${x - 74} ${y - 10}l101 -17 50 30 -102 18Z`}
                />
                <g className={styles.solid}>
                  <path
                    className={styles.faceLeft}
                    d={`M${x - 62} ${y - height - 9}l38 24v${height}l-38 -24Z`}
                  />
                  <path
                    className={styles.faceRight}
                    d={`M${x - 24} ${y - height + 15}l88 -14v${height}l-88 14Z`}
                  />
                  <path
                    className={styles.faceTop}
                    d={`M${x - 62} ${y - height - 9}l88 -14 38 24 -88 15Z`}
                  />
                </g>
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
                  ...tint(kind.kind),
                  left: `${x / 12}%`,
                  top: `${((y - height - 80) / layout.height) * 100}%`,
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
                    <Icon size={17} weight="duotone" aria-hidden />
                    {plural(humanLabel(kind.kind))}
                  </span>
                  <span className={styles.nodeCount}>
                    {kind.entities.toLocaleString("en-AU")}{" "}
                    {plural("object", kind.entities)}
                  </span>
                </button>
                {pins.some((p) => p.kind === kind.kind) && (
                  <div className={styles.attentionPins}>
                    {(["proposal", "stale"] as const).map((type) => {
                      const group = pins.filter(
                        (p) => p.kind === kind.kind && p.type === type,
                      );
                      const pin = group[0];
                      if (!pin) return null;
                      const count = group.reduce(
                        (sum, p) => sum + (p.count ?? 1),
                        0,
                      );
                      const name = `${plural(humanLabel(kind.kind))}: ${count} ${type === "proposal" ? plural("proposal", count) + " to review" : plural("stale input", count)}. ${pin.label}`;
                      const PinIcon =
                        type === "proposal"
                          ? ChatCircleDotsIcon
                          : ClockCountdownIcon;
                      return (
                        <Link
                          key={type}
                          href={pin.href}
                          className={styles.attentionPin}
                          aria-label={name}
                          title={name}
                          data-pin-type={type}
                        >
                          <PinIcon size={16} weight="bold" aria-hidden />
                          <span>{count}</span>
                        </Link>
                      );
                    })}
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
            {labelledRoutes.map(({ link, key, x, y }) => (
              <button
                key={key}
                className={styles.routeLabel}
                style={{
                  ...tint(link.from_kind),
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
            ? ` · ${labelledRoutes.length} routes labelled, all routes in the linked panel`
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
            <div className={styles.inspectorGrid}>
              <div>
                <p className={styles.caption}>
                  {focusKind
                    ? `${focusKind.entities.toLocaleString("en-AU")} objects · Definition: ${focusKind.status === "observed" ? "used, not reviewed" : focusKind.status}`
                    : `${focusLink?.count.toLocaleString("en-AU")} recorded links`}
                </p>
                <div className={styles.sampleChips} aria-label="Examples">
                  {(focusKind
                    ? focusKind.samples.slice(0, 3)
                    : focusLink?.samples
                        .slice(0, 2)
                        .map((s) => `${s.from} to ${s.to}`)
                  )?.map((sample) => (
                    <span key={sample}>{sample}</span>
                  ))}
                  {!(
                    focusKind?.samples.length || focusLink?.samples.length
                  ) && <span>No named examples returned</span>}
                </div>
                <Link
                  className="btn btn-primary btn-sm"
                  href={twinHref("/ontology/objects", space, {
                    kind: focusKind?.kind ?? focusLink?.from_kind ?? "",
                  })}
                >
                  Open{" "}
                  {plural(
                    humanLabel(focusKind?.kind ?? focusLink?.from_kind ?? ""),
                  )}{" "}
                  <ArrowRightIcon size={14} aria-hidden />
                </Link>
              </div>
              {(["in", "out"] as const).map((direction) => {
                const items = detailLinks.filter((l) =>
                  direction === "in"
                    ? l.to_kind === (focusKind?.kind ?? focusLink?.to_kind)
                    : l.from_kind === (focusKind?.kind ?? focusLink?.from_kind),
                );
                return (
                  <section key={direction} aria-label={`Links ${direction}`}>
                    <h4>
                      Links {direction}{" "}
                      <span className={styles.nodeCount}>{items.length}</span>
                    </h4>
                    <ul className={styles.detailLinks}>
                      {items.map((l) => (
                        <li key={linkKey(l)}>
                          <span>
                            {humanLabel(l.label)}{" "}
                            <b>
                              {humanLabel(
                                direction === "in" ? l.from_kind : l.to_kind,
                              )}
                            </b>
                          </span>
                          <span className={styles.nodeCount}>
                            {l.count.toLocaleString("en-AU")}
                          </span>
                          <svg
                            viewBox="0 0 100 3"
                            preserveAspectRatio="none"
                            aria-hidden
                          >
                            <Bar
                              width={linkScale(l.count)}
                              height={3}
                              fill={hue(l.from_kind)}
                            />
                          </svg>
                        </li>
                      ))}
                    </ul>
                    {!items.length && (
                      <p className={styles.caption}>No recorded links</p>
                    )}
                  </section>
                );
              })}
            </div>
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
            {[...kinds]
              .sort(
                (a, b) =>
                  b.entities - a.entities || a.kind.localeCompare(b.kind),
              )
              .map((kind) => (
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
                  <svg
                    viewBox="0 0 100 5"
                    preserveAspectRatio="none"
                    aria-hidden
                  >
                    <Bar
                      x={0}
                      y={0}
                      width={objectScale(kind.entities)}
                      height={5}
                      fill={hue(kind.kind)}
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
                    fill={hue(l.from_kind)}
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
