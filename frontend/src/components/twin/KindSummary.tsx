"use client";
import {
  useEffect,
  useCallback,
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
  XIcon,
} from "@phosphor-icons/react";
import type { OntologyOut } from "@/lib/queries/ontology";
import { useKindColor } from "@/lib/kindColors";
import { twinHref } from "./model";
import { businessSummary } from "./overview-model";
import { humanLabel, plural, linkSentence } from "./overview-format";
import { linkKey, overviewLayout, routeLabels } from "./overview-layout";
import LandscapeLandmark from "./LandscapeLandmark";
import LandscapePlane from "./LandscapePlane";
import styles from "./overview.module.css";
import { useSetOverviewHighlight } from "./OverviewHighlight";

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
export type Highlight =
  | { type: "kind"; key: string }
  | { type: "link"; key: string };

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
  const issuesByKind = useMemo(() => {
    const groups = new Map<string, LandscapePin[]>();
    for (const pin of pins)
      groups.set(pin.kind, [...(groups.get(pin.kind) ?? []), pin]);
    return groups;
  }, [pins]);
  const issueCount = (kind: string) =>
    (issuesByKind.get(kind) ?? []).reduce(
      (sum, pin) => sum + (pin.count ?? 1),
      0,
    );
  const layout = useMemo(
    () => overviewLayout(kinds, links, new Set(issuesByKind.keys())),
    [kinds, links, issuesByKind],
  );
  const overflowIssues = layout.overflow.reduce(
    (sum, k) => sum + issueCount(k.kind),
    0,
  );
  const totalIssues = pins.reduce((sum, pin) => sum + (pin.count ?? 1), 0);
  const unplacedIssues = pins.filter(
    (p) => !kinds.some((k) => k.kind === p.kind),
  );
  const unlinkedArea = useMemo(() => {
    const unlinkedNodes = layout.nodes.filter((n) => !n.linked);
    return unlinkedNodes.length
      ? {
          left: Math.min(...unlinkedNodes.map((n) => n.x)) - 94,
          right: Math.max(...unlinkedNodes.map((n) => n.x)) + 94,
          top: Math.min(...unlinkedNodes.map((n) => n.y)) - 34,
          bottom: Math.max(...unlinkedNodes.map((n) => n.y)) + 32,
        }
      : null;
  }, [layout]);
  const [showAll, setShowAll] = useState(false);
  const [attentionOnly, setAttentionOnly] = useState(false);
  const [allBars, setAllBars] = useState(false);
  const rankedKinds = useMemo(
    () =>
      [...kinds].sort(
        (a, b) => b.entities - a.entities || a.kind.localeCompare(b.kind),
      ),
    [kinds],
  );
  const chartKinds = rankedKinds.filter(
    (k, i) => allBars || i < 12 || issuesByKind.has(k.kind),
  );
  const [moreOpen, setMoreOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [pinGroup, setPinGroup] = useState<{
    kind: string;
    type: LandscapePin["type"];
  } | null>(null);
  const [table, setTable] = useState(false);
  const [hovered, setHovered] = useState<Highlight | null>(null);
  const [focused, setFocused] = useState<Highlight | null>(null);
  const [selected, setSelected] = useState<Highlight | null>(null);
  const active = selected ?? focused ?? hovered;
  const pinIssuesRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (pinGroup) pinIssuesRef.current?.focus();
  }, [pinGroup]);
  const nodeRefs = useRef(new Map<string, HTMLButtonElement>());
  const color = useKindColor();
  const hue = useCallback(
    (kind: string) =>
      families[kind] === "company"
        ? "var(--vault-company)"
        : color(families[kind] ?? kind),
    [color],
  );
  const tint = useCallback(
    (kind: string) => ({ "--kind-hue": hue(kind) }) as CSSProperties,
    [hue],
  );
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
  const choose = useCallback((item: Highlight) => {
    setSelected((previous) =>
      previous?.type === item.type && previous.key === item.key ? null : item,
    );
    setHovered(null);
    setFocused(null);
  }, []);
  const keyNode = useCallback(
    (event: KeyboardEvent<HTMLButtonElement>, kind: string) => {
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
    },
    [layout],
  );
  const nodeNames = useMemo(
    () =>
      new Map(
        kinds.map((kind) => {
          const related = links.filter(
            (l) => l.from_kind === kind.kind || l.to_kind === kind.kind,
          );
          return [
            kind.kind,
            `${plural(humanLabel(kind.kind))}, ${kind.entities.toLocaleString("en-AU")} ${plural("object", kind.entities)}. ${related.length ? related.map(linkSentence).join(". ") : "Not linked yet"}`,
          ];
        }),
      ),
    [kinds, links],
  );
  const detailLinks = focusKind
    ? links.filter(
        (l) => l.from_kind === focusKind.kind || l.to_kind === focusKind.kind,
      )
    : focusLink
      ? [focusLink]
      : [];
  const labelledRoutes = useMemo(
    () =>
      routeLabels(
        layout,
        selected
          ? layout.edges.filter((e) =>
              selected.type === "kind"
                ? e.link.from_kind === selected.key ||
                  e.link.to_kind === selected.key
                : e.key === selected.key,
            )
          : layout.edges,
      ),
    [layout, selected],
  );
  const token = (item: Highlight | null) =>
    item?.type === "kind"
      ? `k${kinds.findIndex((k) => k.kind === item.key)}`
      : item
        ? (layout.edges.find((e) => e.key === item.key)?.token ?? "")
        : "";
  const interactionCSS = useMemo(() => {
    const root = `[data-map-id="${id}"]`;
    const rule = (
      token: string,
      related: typeof layout.edges,
      names: string[],
    ) => {
      const activeRoot = `${root}[data-highlight="${token}"][data-selection]`;
      return (
        related
          .map(
            (e) =>
              `${activeRoot} [data-route="${e.token}"]{display:block}${activeRoot} [data-route-label="${e.token}"]{display:flex}`,
          )
          .join("\n") +
        [...new Set(names)]
          .map(
            (name) =>
              `${activeRoot} [data-form="k${kinds.findIndex((k) => k.kind === name)}"]{opacity:1}${activeRoot} [data-form="k${kinds.findIndex((k) => k.kind === name)}"] .${styles.kindTitle} svg{opacity:1}`,
          )
          .join("\n")
      );
    };
    return (
      kinds
        .map((kind, i) => {
          const related = layout.edges.filter(
            (e) =>
              e.link.from_kind === kind.kind || e.link.to_kind === kind.kind,
          );
          return rule(`k${i}`, related, [
            kind.kind,
            ...related.flatMap((e) => [e.link.from_kind, e.link.to_kind]),
          ]);
        })
        .join("\n") +
      layout.edges
        .map((e) => rule(e.token, [e], [e.link.from_kind, e.link.to_kind]))
        .join("\n") +
      kinds
        .map(
          (_, i) =>
            `${root}[data-selection="k${i}"] [data-form="k${i}"] .${styles.solid}{transform:translateY(-9px);filter:brightness(1.17)} ${root}[data-selection="k${i}"] [data-form="k${i}"] .${styles.baseRing}{opacity:1}`,
        )
        .join("\n")
    );
  }, [id, kinds, layout]);
  const visibleRoutes = layout.edges.filter((e) =>
    selected
      ? relevantLink(e.key, e.link.from_kind, e.link.to_kind)
      : showAll ||
        e.idle ||
        !!(active && relevantLink(e.key, e.link.from_kind, e.link.to_kind)),
  ).length;
  return (
    <section
      className={styles.map}
      data-map-id={id}
      data-highlight={token(active)}
      data-selection={token(selected)}
      data-show-all={showAll}
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
      <style>{interactionCSS}</style>
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
      <button
        className={`btn btn-ghost btn-sm ${styles.routeToggle}`}
        aria-pressed={showAll}
        onClick={() => setShowAll((value) => !value)}
      >
        Show all routes
      </button>
      <div className={styles.heightLegend}>
        <span aria-hidden>▂ ▄ █</span> height = objects{" "}
        <span className={styles.caption}>
          Square-root scale · equal footprints
        </span>
      </div>
      {totalIssues > 0 && (
        <section className={styles.mapNotices} aria-label="Owner attention">
          <span className={styles.issueCount}>
            {totalIssues} {plural("issue", totalIssues)} need attention
          </span>
          {unplacedIssues.map((p) => (
            <Link key={p.href} href={p.href}>
              {p.label}
            </Link>
          ))}
        </section>
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
              {kinds.map((kind) => (
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
                      {linkSentence(l)}
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
          <LandscapePlane
            id={id}
            layout={layout}
            kinds={kinds}
            unlinkedArea={unlinkedArea}
            tint={tint}
          />
          {layout.nodes.map((node) => (
            <LandscapeLandmark
              key={node.kind.kind}
              node={node}
              token={`k${kinds.findIndex((k) => k.kind === node.kind.kind)}`}
              planeHeight={layout.height}
              firstUnlinked={layout.unlinked[0]?.kind}
              links={links}
              pins={pins}
              name={nodeNames.get(node.kind.kind) ?? ""}
              selected={
                selected?.type === "kind" && selected.key === node.kind.kind
              }
              tint={tint}
              nodeRefs={nodeRefs}
              choose={choose}
              keyNode={keyNode}
              setHovered={setHovered}
              setFocused={setFocused}
              setPinGroup={setPinGroup}
            />
          ))}
          <div className={styles.routeLabels}>
            {labelledRoutes.map(({ link, key, x, y, angle, token, idle }) => (
              <button
                key={key}
                className={styles.routeLabel}
                data-route-label={token}
                data-idle={idle}
                data-arrow-angle={angle}
                title={linkSentence(link)}
                aria-label={linkSentence(link)}
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
                <span>{linkSentence(link)}</span>
                <ArrowRightIcon
                  aria-hidden
                  size={12}
                  style={{ transform: `rotate(${angle}deg)` }}
                />
              </button>
            ))}
          </div>
        </div>
      )}
      {layout.overflow.length > 0 && (
        <div className={styles.moreKinds}>
          <button
            className="btn btn-ghost"
            aria-expanded={moreOpen}
            onClick={() => {
              if (!moreOpen) setAttentionOnly(overflowIssues > 0);
              setMoreOpen((value) => !value);
            }}
          >
            {layout.overflow.length} more kinds
            {overflowIssues > 0 && (
              <span
                className={styles.issueCount}
                data-issue-count={overflowIssues}
              >
                {" "}
                · {overflowIssues} {plural("issue", overflowIssues)}
              </span>
            )}
          </button>
          {moreOpen && (
            <section aria-label="More kinds">
              <label className={styles.attentionFilter}>
                <input
                  type="checkbox"
                  checked={attentionOnly}
                  onChange={(e) => setAttentionOnly(e.target.checked)}
                />
                Needs attention only
              </label>
              <label>
                Find a kind
                <input
                  type="search"
                  aria-label="Find a kind"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                />
              </label>
              <ul>
                {layout.overflow
                  .filter((k) => !attentionOnly || issueCount(k.kind) > 0)
                  .filter((k) =>
                    [k.kind, humanLabel(k.kind)].some((name) =>
                      name
                        .toLocaleLowerCase()
                        .includes(search.toLocaleLowerCase()),
                    ),
                  )
                  .map((k) => (
                    <li key={k.kind}>
                      <button
                        className="link"
                        onClick={() => choose({ type: "kind", key: k.kind })}
                      >
                        {humanLabel(k.kind)}
                      </button>{" "}
                      <span className={styles.nodeCount}>
                        {k.entities.toLocaleString("en-AU")} objects
                      </span>{" "}
                      <span
                        className={
                          issueCount(k.kind)
                            ? styles.issueCount
                            : styles.caption
                        }
                        data-issue-count={issueCount(k.kind)}
                      >
                        {issueCount(k.kind)}{" "}
                        {plural("issue", issueCount(k.kind))}
                      </span>{" "}
                      <Link
                        className="link"
                        href={
                          issuesByKind.get(k.kind)?.[0]?.href ??
                          twinHref("/ontology/review", space)
                        }
                      >
                        Review {humanLabel(k.kind)}
                      </Link>
                    </li>
                  ))}
              </ul>
            </section>
          )}
        </div>
      )}
      {pinGroup && (
        <section
          className={styles.pinIssues}
          ref={pinIssuesRef}
          tabIndex={-1}
          aria-label={`${humanLabel(pinGroup.kind)} issues`}
        >
          <div className={styles.sectionHead}>
            <h3>{humanLabel(pinGroup.kind)} issues</h3>
            <button
              className="btn btn-ghost btn-sm"
              onClick={() => {
                nodeRefs.current.get(pinGroup.kind)?.focus();
                setPinGroup(null);
              }}
            >
              Close issues
            </button>
          </div>
          <ul>
            {pins
              .filter(
                (p) => p.kind === pinGroup.kind && p.type === pinGroup.type,
              )
              .map((p) => (
                <li key={p.href}>
                  <Link className="link" href={p.href}>
                    {p.label}
                  </Link>
                </li>
              ))}
          </ul>
        </section>
      )}
      <div className={styles.mapFoot}>
        <span>
          {kinds.length} {plural("kind", kinds.length)} · {links.length}{" "}
          recorded link types
          <span className={styles.planeCaption}>
            {table
              ? ` · All ${links.length} recorded relationships listed`
              : ` · ${visibleRoutes} of ${links.length} routes shown`}
          </span>
          <span className={styles.flatCaption}>
            {" "}
            · All {links.length} recorded relationships listed
            {layout.overflow.length > 0 ? " in Recorded links below" : ""}
          </span>
          {layout.overflow.length > 0 &&
            ` · ${layout.nodes.length} kinds on the landscape; all ${kinds.length} in the table`}
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
                  className={`btn btn-primary btn-sm ${styles.inspectorAction}`}
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
                {focusKind && (
                  <section
                    aria-label={`${humanLabel(focusKind.kind)} pending issues`}
                    className={styles.kindIssues}
                  >
                    <h4>
                      {issueCount(focusKind.kind)}{" "}
                      {plural("issue", issueCount(focusKind.kind))} pending
                    </h4>
                    <ul>
                      {(issuesByKind.get(focusKind.kind) ?? []).map((p) => (
                        <li key={p.href}>
                          <Link className="link" href={p.href}>
                            {p.label}
                            {p.count
                              ? ` (${p.count} ${plural("stale value", p.count)})`
                              : ""}
                          </Link>
                        </li>
                      ))}
                    </ul>
                  </section>
                )}
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
                          <span>{linkSentence(l)}</span>
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
            {chartKinds.map((kind) => (
              <button
                key={kind.kind}
                className={styles.barRow}
                data-objects={kind.entities}
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
                    fill={hue(kind.kind)}
                    rx={1}
                  />
                </svg>
              </button>
            ))}
          </div>
          {kinds.length > 12 && (
            <button
              className="btn btn-ghost btn-sm"
              aria-expanded={allBars}
              onClick={() => setAllBars((value) => !value)}
            >
              {allBars ? "Show fewer kinds" : `Show all ${kinds.length} kinds`}
            </button>
          )}
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
