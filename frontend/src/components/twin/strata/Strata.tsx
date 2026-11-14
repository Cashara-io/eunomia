"use client";
import { useEffect, useEffectEvent, useMemo, useRef, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { ArrowRightIcon, ListMagnifyingGlassIcon, PlusIcon } from "@phosphor-icons/react";
import { ApiError } from "@/lib/api";
import { useLiveStatus } from "@/lib/queries/live";
import { useConnectionSearch, useNeighbourhood, type ConnectionNode } from "@/lib/queries/connections";
import { useObjectProfile } from "@/lib/queries/objects";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import { useConfirmedSpaceAccess, useMissingSpaceObjects } from "../SpaceAccess";
import { filterScene, type Direction } from "../connections-model";
import { useTwinParams } from "../controls";
import { twinHref } from "../model";
import { humanLabel, plural, relativeTime } from "../overview-format";
import ReadState from "../ReadState";
import KindsStage, { useKindsLayout } from "./KindsStage";
import ObjectsStage, { objectsWires } from "./ObjectsStage";
import StrataReceipt from "./StrataReceipt";
import { AroundInspector, KindInspector, ObjectInspector } from "./StrataInspector";
import { KindsTables, ObjectsTables } from "./StrataTables";
import StrataRail from "./StrataRail";
import StrataSkeleton from "./StrataSkeleton";
import { buildLanes, kindRows } from "./strata-model";
import { useLatestActivity } from "./use-latest-activity";
import Wires from "./Wires";
import styles from "./strata.module.css";

const subscribeMobile = (listener: () => void) => {
  const media = window.matchMedia("(max-width: 640px)");
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
};
const mobileSnapshot = () => window.matchMedia("(max-width: 640px)").matches;

type Ontology = { data?: OntologyOut; isError: boolean; error: unknown; refetch: () => unknown };

export default function Strata({ vault, ontology, defaultLevel, onAdd }: {
  vault: Vault;
  ontology: Ontology;
  defaultLevel: "kinds" | "objects";
  onAdd: () => void;
}) {
  const space = vault.id;
  const { params, update } = useTwinParams();
  const confirmed = useConfirmedSpaceAccess();
  const missing = useMissingSpaceObjects();
  const mobile = useSyncExternalStore(subscribeMobile, mobileSnapshot, () => false);
  const live = useLiveStatus();
  const data = ontology.data;
  const stage = useRef<HTMLDivElement>(null);

  const rawMode = params.get("mode");
  const level = rawMode === "kinds" || rawMode === "objects" ? rawMode : defaultLevel;
  const focus = params.get("focus") ?? "";
  const selected = params.get("selected") ?? "";
  const value = params.get("value") ?? "";
  const search = params.get("q") ?? "";
  const kind = params.get("kind") ?? "";
  const open = params.get("open") ?? "";
  const depth = Math.min(3, Math.max(1, Number(params.get("depth")) || 1));
  const relation = params.get("relation") ?? "";
  const rawDirection = params.get("direction");
  const direction: Direction = rawDirection === "in" || rawDirection === "out" ? rawDirection : "both";
  const table = params.get("view") === "table";
  const offset = Math.max(0, Number(params.get("offset")) || 0);
  const objects = level === "objects";

  const hood = useNeighbourhood(space, objects ? focus : "", depth);
  const results = useConnectionSearch(space, search, kind, offset, objects && (!!search.trim() || (!focus && !!kind)));
  const suggestions = useConnectionSearch(space, "", kind, 0, objects && !search.trim());
  const [recentNodes, setRecent] = useState<ConnectionNode[]>([]);
  const recent = recentNodes.filter((n) => !missing.has(n.id));
  const scene = useMemo(
    () =>
      hood.data
        ? filterScene(
            {
              ...hood.data,
              nodes: hood.data.nodes.filter((n) => !missing.has(n.id)),
              edges: hood.data.edges.filter((e) => !missing.has(e.from) && !missing.has(e.to)),
            },
            focus,
            relation,
            direction,
            depth,
          )
        : null,
    [hood.data, focus, relation, direction, depth, missing],
  );
  const nodes = useMemo(() => new Map(scene?.nodes.map((n) => [n.id, n])), [scene]);
  const lanes = useMemo(() => (scene ? buildLanes(scene, focus) : []), [scene, focus]);
  const focusNode = nodes.get(focus);
  const selectedNode = nodes.get(selected);
  const focusProfile = useObjectProfile(space, focusNode ? focus : "");
  const latest = useLatestActivity(space, confirmed);
  const [now] = useState(() => Date.now());
  const [filtersOpen, setFiltersOpen] = useState(false);
  const denied = hood.error instanceof ApiError && [403, 404].includes(hood.error.status);
  const kindsLayout = useKindsLayout(data, kind, mobile);
  const waiting = useMemo(() => (data ? kindRows(data).reduce((n, r) => n + r.waiting, 0) : 0), [data]);

  const escape = useEffectEvent((event: globalThis.KeyboardEvent) => {
    if (!confirmed || event.key !== "Escape") return;
    if (value) update({ value: "" });
    else if (selected) update({ selected: "" });
  });
  useEffect(() => {
    const listener = (event: globalThis.KeyboardEvent) => escape(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);
  useEffect(() => {
    if (!confirmed || !missing.size) return;
    if (missing.has(focus)) update({ focus: "", selected: "", value: "" }, true);
    else if (missing.has(selected)) update({ selected: "", value: "" }, true);
  }, [confirmed, missing, focus, selected, update]);

  function remember(node: ConnectionNode) {
    setRecent((prev) => [node, ...prev.filter((n) => n.id !== node.id)].slice(0, 4));
  }
  const focusOn = (node: ConnectionNode, select = "") => {
    remember(node);
    update({ mode: "objects", focus: node.id, selected: select, value: "", open: "", q: "", offset: "0" });
  };
  const selectNode = (id: string) => {
    const node = nodes.get(id);
    if (node) remember(node);
    update({ selected: id, value: "" });
  };
  const selectKind = (next: string) =>
    update({ mode: "kinds", kind: next === kind ? "" : next, selected: "", value: "", open: "", q: "" });
  const closeNode = () => {
    const id = selected;
    update({ selected: "", value: "" });
    queueMicrotask(() => document.querySelector<HTMLElement>(`[data-object="${CSS.escape(id)}"]`)?.focus());
  };
  const closeReceipt = () => {
    const name = value;
    update({ value: "" });
    queueMicrotask(() => document.querySelector<HTMLElement>(`[data-wire="value:${CSS.escape(name)}"]`)?.focus());
  };

  const wires = useMemo(() => {
    if (!confirmed || table) return [];
    if (!objects) {
      if (!kindsLayout) return [];
      return kindsLayout.wires;
    }
    if (!focusNode) return [];
    const tethers = objectsWires(lanes, focus, focusNode.kind, selected, !mobile && !!selectedNode);
    if (!mobile && selectedNode && value)
      tethers.push({ id: "receipt", from: `value:${value}`, to: "receipt", fromSide: "l", toSide: "r", tone: "tether" });
    return tethers;
  }, [confirmed, table, objects, kindsLayout, focusNode, lanes, focus, selected, selectedNode, value, mobile]);

  const header = (
    <div className={styles.toolbar}>
      <div className={styles.level} role="group" aria-label="Level">
        <button aria-pressed={!objects} onClick={() => update({ mode: "kinds", selected: "", value: "" })}>
          Kinds
        </button>
        <button aria-pressed={objects} onClick={() => update({ mode: "objects" })}>
          Objects
        </button>
      </div>
      <label className={styles.search}>
        <span className="sr-only">{objects ? "Find an object" : "Find a kind"}</span>
        <input
          type="search"
          autoComplete="off"
          className="field"
          placeholder={objects ? "Find an object by name" : "Find a kind"}
          value={search}
          onChange={(e) => update({ q: e.target.value, offset: "0" }, true)}
        />
      </label>
      {objects && focus && (
        <>
          <button
            className={`btn btn-sm ${styles.filterToggle}`}
            aria-expanded={filtersOpen}
            aria-controls="strata-filters"
            onClick={() => setFiltersOpen((v) => !v)}
          >
            Filters
          </button>
          <div id="strata-filters" className={styles.filters} data-open={filtersOpen}>
            <label>
              <span>Depth</span>
              <select aria-label="Depth" value={depth} onChange={(e) => update({ depth: e.target.value })}>
                {[1, 2, 3].map((n) => (
                  <option key={n} value={n}>
                    {n} {n === 1 ? "hop" : "hops"}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>Relation</span>
              <select aria-label="Relation" value={relation} onChange={(e) => update({ relation: e.target.value })}>
                <option value="">All relations</option>
                {[...new Set([...(hood.data?.edges.map((e) => e.label) ?? []), ...(relation ? [relation] : [])])]
                  .sort()
                  .map((r) => (
                    <option key={r} value={r}>
                      {humanLabel(r)}
                    </option>
                  ))}
              </select>
            </label>
            <label>
              <span>Direction</span>
              <select aria-label="Direction" value={direction} onChange={(e) => update({ direction: e.target.value })}>
                <option value="both">In and out</option>
                <option value="in">Incoming to focus</option>
                <option value="out">Outgoing from focus</option>
              </select>
            </label>
          </div>
        </>
      )}
      <span className={styles.grow} />
      <button className="btn btn-sm" aria-pressed={table} onClick={() => update({ view: table ? "canvas" : "table" })}>
        <ListMagnifyingGlassIcon size={16} aria-hidden />
        {table ? "Show canvas" : "Show as table"}
      </button>
    </div>
  );

  let canvas;
  let inspector;
  if (!confirmed || (!data && !ontology.isError)) {
    canvas = <StrataSkeleton objects={objects} />;
  } else if (!data) {
    canvas = (
      <div className={styles.alert}>
        <ReadState error={ontology.error} retry={() => void ontology.refetch()} subject="the ontology" />
      </div>
    );
  } else if (rowsEmpty(data)) {
    inspector = <p className={styles.hint}>Kinds, links and sources appear here once records are mapped.</p>;
    canvas = (
      <div className={styles.state}>
        <svg viewBox="0 0 340 150" aria-hidden className={styles.motif}>
          <path d="M40 118 210 85 310 125 140 148Z" fill="var(--accent-soft)" />
          <path d="m60 80 65-12 34 20-65 12Z" fill="var(--kind-repository)" />
          <path d="m60 80 34 20v27L60 107Z" fill="var(--border-field)" />
          <path d="m94 100 65-12v27l-65 12Z" fill="var(--ink-dim)" />
          <path d="m160 112 85-17" fill="none" stroke="var(--ink-dim)" strokeDasharray="4 5" />
          <circle cx="255" cy="92" r="8" fill="var(--surface)" stroke="var(--ink-dim)" />
        </svg>
        <h2>Start with the things your business knows.</h2>
        <p>
          Map your records into kinds and relationships. Then review the suggestions. A product, its supplier, an order:
          add a CSV or connected records to start mapping how they fit together.
        </p>
        {vault.role === "owner" ? (
          <button className="btn btn-primary" onClick={onAdd}>
            <PlusIcon size={16} aria-hidden /> Add data
          </button>
        ) : (
          <p>Ask a space owner to add data.</p>
        )}
      </div>
    );
  } else if (!objects) {
    canvas = table ? (
      <KindsTables data={data} space={space} select={selectKind} />
    ) : (
      kindsLayout && <KindsStage layout={kindsLayout} selected={kind} query={search} mobile={mobile} select={selectKind} />
    );
    inspector = (
      <KindInspector
        vault={vault}
        data={data}
        kind={kind}
        selectKind={selectKind}
        openObjects={(k) => update({ mode: "objects", kind: k, focus: "", selected: "", value: "", q: "", open: "" })}
      />
    );
  } else if (!focus) {
    inspector = <p className={styles.hint}>Pick an object to open its neighbourhood. Its values and sources appear here.</p>;
    canvas = (
      <ObjectPicker
        search={search}
        kind={kind}
        offset={offset}
        results={results}
        suggestions={suggestions}
        recent={recent}
        missing={missing}
        showStart={!focus}
        pick={(n) => focusOn(n, n.id)}
        update={update}
      />
    );
  } else if (hood.isError && !scene) {
    canvas = (
      <div className={styles.alert}>
        <p className={styles.alertTitle}>{denied ? "This object is unavailable here." : "The connections could not be loaded."}</p>
        <ReadState error={hood.error} retry={() => void hood.refetch()} subject="this neighbourhood" />
      </div>
    );
  } else if (!scene) {
    canvas = <StrataSkeleton objects />;
  } else if (!focusNode) {
    canvas = (
      <div className={styles.state}>
        <h2>No object returned.</h2>
        <p>Search for another object in this space.</p>
      </div>
    );
  } else if (table) {
    canvas = <ObjectsTables scene={scene} nodes={nodes} selected={selected} select={selectNode} />;
  } else {
    canvas = (
      <>
        {(hood.data?.truncated || hood.data?.partial) && (
          <p className={styles.limit} role="status">
            Partial neighbourhood. The service returns at most 150 objects and 600 links, with a per-object limit. More
            may exist. Paging is not available from this service yet. Filters and counts describe only these loaded results.
          </p>
        )}
        {hood.isError && <ReadState error={hood.error} retry={() => void hood.refetch()} subject="this neighbourhood" />}
        {!scene.edges.length && (
          <p className={styles.limit}>
            {hood.data?.edges.length
              ? "No links match these filters within this loaded neighbourhood."
              : "This object is disconnected in this neighbourhood. No links were returned within the selected depth."}
          </p>
        )}
        <ObjectsStage
          focusNode={focusNode}
          focusProfile={focusProfile.data}
          lanes={lanes}
          selected={selected}
          open={open}
          mobile={mobile}
          select={selectNode}
          toggle={(id) => update({ open: open === id ? "" : id })}
          setKind={selectKind}
        />
      </>
    );
  }
  if (confirmed && !data && !ontology.isError) {
    inspector = <InspectorBones />;
  } else if (data && objects && focus && !search.trim() && !focusNode && !table) {
    const profile = focusProfile.data;
    inspector = hood.isError && profile ? (
      <ObjectInspector
        key={focus}
        space={space}
        node={{ id: focus, name: profile.name, kind: profile.kind, facts: [] }}
        focus={focus}
        focusName={profile.name}
        value={value}
        selectValue={(name) => update({ value: name })}
        selectNode={selectNode}
        makeFocus={() => {}}
        close={() => update({ focus: "", selected: "", value: "" })}
        mobile={mobile}
      />
    ) : hood.isError ? null : <InspectorBones />;
  } else if (data && objects && focusNode && scene) {
    inspector = selectedNode ? (
      <ObjectInspector
        key={selectedNode.id}
        space={space}
        node={selectedNode}
        focus={focus}
        focusName={focusNode.name}
        value={value}
        selectValue={(name) => update({ value: name })}
        selectNode={selectNode}
        makeFocus={() => focusOn(selectedNode)}
        close={closeNode}
        mobile={mobile}
      />
    ) : (
      <AroundInspector
        focusName={focusNode.name}
        lanes={lanes}
        nodes={scene.nodes.length}
        edges={scene.edges.length}
        partial={!!(hood.data?.truncated || hood.data?.partial)}
        select={selectNode}
        pickLane={(id) => update({ open: id })}
      />
    );
  }
  const floating =
    confirmed && data && objects && !!focus && !!search.trim() && !table ? (
      <div className={styles.floating}>
        <ObjectPicker
          search={search}
          kind={kind}
          offset={offset}
          results={results}
          suggestions={suggestions}
          recent={recent}
          missing={missing}
          showStart={false}
          pick={(n) => focusOn(n, n.id)}
          update={update}
        />
      </div>
    ) : null;
  const receiptOpen = !mobile && !table && objects && !!selectedNode && !!value;

  return (
    <section className={styles.stage} aria-label={objects ? "Objects connections" : "Kinds connections"} ref={stage} data-level={level}>
      {header}
      <StrataRail data={data} level={level} kind={kind} confirmed={confirmed} select={selectKind} space={space} waiting={waiting} focusKind={focusNode?.kind} recent={recent} pick={(n) => focusOn(n, "")} />
      <div className={styles.canvasWrap}>
        {confirmed && missing.size > 0 && (
          <p role="status" className={styles.missing}>
            This object no longer exists
          </p>
        )}
        {floating}
        <div
          className={styles.canvas}
          role="group"
          aria-label={objects ? "Object connections canvas" : "Kinds canvas"}
          tabIndex={-1}
        >
          {canvas}
        </div>
        <div className={styles.strip} role="status" aria-live="polite">
          <span className={styles.live} data-status={live}>
            {live === "live" ? "Live" : live === "offline" ? "Offline" : "Connecting"}
          </span>
          <span className={styles.stripText}>
            {confirmed && latest.data
              ? `${latest.data.actor_name} ${latest.data.action} ${humanLabel(latest.data.name)} on ${humanLabel(latest.data.kind)}, ${relativeTime(latest.data.at, now)}.`
              : confirmed && data
                ? `${kindRows(data).length} ${plural("kind", kindRows(data).length)}. ${waiting} ${plural("definition", waiting)} in review.`
                : "Reading the ontology."}
          </span>
        </div>
      </div>
      <aside className={styles.inspector} aria-label={confirmed && selectedNode ? "Selected object" : "Inspector"} data-wire="inspector" data-open={!!selectedNode || (!objects && !!kind)}>
        <div className={styles.grip} aria-hidden />
        {confirmed ? inspector : null}
      </aside>
      {receiptOpen && selectedNode && (
        <StrataReceipt space={space} id={selectedNode.id} name={value} close={closeReceipt} inline={false} />
      )}
      {!mobile && <Wires stage={stage} wires={wires} receipt={receiptOpen ? { from: `value:${value}`, inspector: "inspector" } : undefined} />}
    </section>
  );
}

function InspectorBones() {
  return (
    <div aria-hidden className={styles.bones}>
      {[60, 90, 100, 100, 80].map((w, i) => (
        <div key={i} className={styles.bone} style={{ width: `${w}%`, minHeight: i === 0 ? "1.5rem" : "2.5rem" }} />
      ))}
    </div>
  );
}

function rowsEmpty(data: OntologyOut) {
  return kindRows(data).length === 0;
}

import type { UseQueryResult } from "@tanstack/react-query";
type Search = UseQueryResult<{ results: ConnectionNode[]; has_more?: boolean }>;
function ObjectPicker({ search, kind, offset, results, suggestions, recent, missing, showStart, pick, update }: {
  search: string;
  kind: string;
  offset: number;
  results: Search;
  suggestions: Search;
  recent: ConnectionNode[];
  missing: ReadonlySet<string>;
  showStart: boolean;
  pick: (node: ConnectionNode) => void;
  update: (values: Record<string, string>, replace?: boolean) => void;
}) {
  const searching = !!search.trim() || (showStart && !!kind);
  return (
    <div className={styles.picker}>
      {searching && (
        <section className={styles.results} aria-label="Object search results">
          {kind && (
            <button className="btn btn-sm" onClick={() => update({ kind: "", offset: "0" })}>
              All kinds, now showing {humanLabel(kind)}
            </button>
          )}
          {results.isError ? (
            <ReadState error={results.error} retry={() => void results.refetch()} subject="matching objects" />
          ) : !results.data ? (
            <p role="status">Finding objects…</p>
          ) : (
            <>
              {results.data.results
                .filter((n) => !missing.has(n.id))
                .map((n) => (
                  <button className={styles.result} key={n.id} onClick={() => pick(n)}>
                    <strong>{n.name}</strong>
                    <span>{humanLabel(n.kind)}</span>
                  </button>
                ))}
              {!results.data.results.length && <p>No objects match this search in this space. Try another name.</p>}
              <div className={styles.pager}>
                <span>{results.data.results.length} results on this page</span>
                <button className="btn btn-sm" disabled={offset === 0} onClick={() => update({ offset: String(Math.max(0, offset - 20)) })}>
                  Previous results
                </button>
                <button
                  className="btn btn-sm"
                  disabled={results.data.has_more === false || results.data.results.length < 20}
                  onClick={() => update({ offset: String(offset + 20) })}
                >
                  Next results
                </button>
              </div>
            </>
          )}
        </section>
      )}
      {showStart && (
        <div className={styles.state}>
          <svg viewBox="0 0 340 150" aria-hidden className={styles.motif}>
            <path d="M40 118 210 85 310 125 140 148Z" fill="var(--accent-soft)" />
            <path d="m60 80 65-12 34 20-65 12Z" fill="var(--kind-repository)" />
            <path d="m60 80 34 20v27L60 107Z" fill="var(--border-field)" />
            <path d="m94 100 65-12v27l-65 12Z" fill="var(--ink-dim)" />
            <path d="m160 112 85-17" fill="none" stroke="var(--ink-dim)" strokeDasharray="4 5" />
            <circle cx="255" cy="92" r="8" fill="var(--surface)" stroke="var(--ink-dim)" />
          </svg>
          <h2>Start with something you know.</h2>
          <p>Find a product, an order or a place. Its neighbourhood opens here, one hop at a time.</p>
          <p className={styles.hint}>Only objects you can read in this memory space are included.</p>
          {suggestions.data && (
            <div className={styles.suggestions} aria-label="Suggested objects">
              <span>Suggested starting points</span>
              {suggestions.data.results
                .filter((n) => !missing.has(n.id) && !recent.some((r) => r.id === n.id))
                .slice(0, 4)
                .map((n) => (
                  <button className="pill" key={n.id} onClick={() => pick(n)}>
                    {n.name}
                  </button>
                ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
