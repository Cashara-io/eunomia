"use client";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { useOntology } from "@/lib/queries/ontology";
import {
  type ConnectionNode,
  useConnectionSearch,
  useNeighbourhood,
} from "@/lib/queries/connections";
import { ApiError } from "@/lib/api";
import { ConnectionContent, useConfirmedConnectionAccess, useMissingConnectionObjects } from "./ConnectionAccess";
import { useTwinParams } from "./controls";
import { twinHref } from "./model";
import { humanLabel, plural } from "./overview-format";
import KindSummary from "./KindSummary";
import ConnectionCanvas, { type ConnectionCanvasInput } from "./ConnectionCanvas";
import ConnectionSummary from "./ConnectionSummary";
import ConnectionInspector from "./ConnectionInspector";
import ReadState from "./ReadState";
import { edgeSentence, filterScene, type Direction } from "./connections-model";
import styles from "./connections.module.css";

export default function Connections({ space }: { space: string }) {
  const { params, update } = useTwinParams();
  const mode = params.get("mode") === "kinds" ? "kinds" : "objects";
  return (
    <section
      className={styles.root}
      aria-label={`${mode === "kinds" ? "Kinds" : "Objects"} connections`}
    >
      <ConnectionContent>
      <header className={styles.spread}>
        <div>
          <h2>Follow an object and the records that connect it.</h2>
        </div>
        <Link className="link" href={twinHref("/ontology", space)}>
          Back to Overview
        </Link>
      </header>
      <div className={styles.modes} role="group" aria-label="Connection mode">
        <button
          className="pill"
          aria-pressed={mode === "kinds"}
          onClick={() => update({ mode: "kinds" })}
        >
          Kinds
        </button>
        <button
          className="pill"
          aria-pressed={mode === "objects"}
          onClick={() => update({ mode: "objects" })}
        >
          Objects
        </button>
        <span>
          {mode === "kinds"
            ? "Types of things, across this space"
            : "Real things, in a local neighbourhood"}
        </span>
      </div>
      </ConnectionContent>
      {mode === "kinds" ? (
        <ConnectionContent><Kinds space={space} /></ConnectionContent>
      ) : (
        <Objects key={space} space={space} />
      )}
    </section>
  );
}
function Kinds({ space }: { space: string }) {
  const query = useOntology(space);
  if (query.isError)
    return (
      <ReadState
        error={query.error}
        retry={() => void query.refetch()}
        subject="kinds"
      />
    );
  if (!query.data) return <p role="status">Loading kinds…</p>;
  return (
    <>
      <p>
        These are kinds, not individual objects. Choose Objects to follow a real
        thing.
      </p>
      <KindSummary data={query.data} space={space} />
    </>
  );
}
function focusObject(id: string) {
  const candidates = document.querySelectorAll<HTMLButtonElement>(
    `[data-object="${CSS.escape(id)}"]`,
  );
  [...candidates].find((el) => el.getClientRects().length > 0)?.focus();
}
const subscribeMobile = (listener: () => void) => {
  const media = window.matchMedia("(max-width: 640px)");
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
};
const mobileSnapshot = () => window.matchMedia("(max-width: 640px)").matches;
function Objects({ space }: { space: string }) {
  const { params, update } = useTwinParams();
  const missing = useMissingConnectionObjects();
  const confirmed = useConfirmedConnectionAccess();
  const [canvasInput, setCanvasInput] = useState<ConnectionCanvasInput | null>(null);
  const mobile = useSyncExternalStore(
    subscribeMobile,
    mobileSnapshot,
    () => false,
  );
  const focus = params.get("focus") ?? "",
    selected = params.get("selected") ?? "";
  const search = params.get("q") ?? "",
    kind = params.get("kind") ?? "";
  const depth = Math.min(3, Math.max(1, Number(params.get("depth")) || 1));
  const relation = params.get("relation") ?? "";
  const rawDirection = params.get("direction");
  const direction: Direction =
    rawDirection === "in" || rawDirection === "out" ? rawDirection : "both";
  const table = params.get("view") === "table";
  const offset = Math.max(0, Number(params.get("offset")) || 0);
  const searchQuery = useConnectionSearch(
    space,
    search,
    kind,
    offset,
    !!search.trim() || (!focus && !!kind),
  );
  const query = useNeighbourhood(space, focus, depth);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [hovered, setHovered] = useState("");
  const [activeKind, setActiveKind] = useState("");
  const [recentNodes, setRecent] = useState<ConnectionNode[]>([]);
  const recent = recentNodes.filter(n => !missing.has(n.id));
  const suggestions = useConnectionSearch(space, "", kind, 0, !search.trim());
  const scene = useMemo(
    () =>
      query.data
        ? filterScene({
            ...query.data,
            nodes: query.data.nodes.filter(n => !missing.has(n.id)),
            edges: query.data.edges.filter(e => !missing.has(e.from) && !missing.has(e.to)),
          }, focus, relation, direction, depth)
        : null,
    [query.data, focus, relation, direction, depth, missing],
  );
  const nodes = new Map(scene?.nodes.map((n) => [n.id, n]));
  const selectedNode = nodes.get(selected);
  const active = hovered || selected;
  const related =
    scene?.edges.filter((e) => e.from === selected || e.to === selected) ?? [];
  const denied =
    query.error instanceof ApiError && [403, 404].includes(query.error.status);
  useEffect(() => {
    function escape(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape" && selected) {
        update({ selected: "" });
        focusObject(selected);
      }
    }
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  });
  useEffect(() => {
    if (!confirmed || !missing.size) return;
    if (missing.has(focus)) update({ focus: "", selected: "" }, true);
    else if (missing.has(selected)) update({ selected: "" }, true);
  }, [confirmed, missing, focus, selected, update]);
  function remember(node: ConnectionNode) {
    setRecent((prev) =>
      [node, ...prev.filter((n) => n.id !== node.id)].slice(0, 4),
    );
  }
  function open(node: ConnectionNode) {
    remember(node);
    update({ focus: node.id, selected: "", q: "", offset: "0" });
  }
  function select(id: string) {
    const node = nodes.get(id);
    if (node) remember(node);
    update({ selected: id });
  }
  function close() {
    update({ selected: "" });
    focusObject(selected);
  }
  if (!confirmed) return null;
  return (
    <div>
      {missing.size > 0 && <p role="status">This object no longer exists</p>}
      <form className={styles.search} onSubmit={(e) => e.preventDefault()}>
        <label>
          <span className="label">
            Find an object{kind ? ` · ${humanLabel(kind)}` : ""}
          </span>
          <input
            autoComplete="off"
            type="search"
            className="field"
            placeholder="Search by name, then choose an object"
            value={search}
            onChange={(e) => update({ q: e.target.value, offset: "0" }, true)}
          />
        </label>
        {kind && (
          <button
            className="btn btn-sm"
            onClick={() => update({ kind: "", offset: "0" })}
          >
            All kinds
          </button>
        )}
      </form>
      {!search.trim() && (
        <>
          {recent.length > 0 && (
            <div
              className={styles.suggestions}
              aria-label="Recently viewed objects"
            >
              <span>Recently viewed</span>
              {recent.map((n) => (
                <button key={n.id} onClick={() => open(n)}>
                  {n.name}
                </button>
              ))}
            </div>
          )}
          {suggestions.data && (
            <div className={styles.suggestions} aria-label="Suggested objects">
              <span>Suggested starting points</span>
              {suggestions.data.results
                .filter((n) => !missing.has(n.id) && !recent.some((r) => r.id === n.id))
                .slice(0, 4)
                .map((n) => (
                  <button key={n.id} onClick={() => open(n)}>
                    {n.name}
                  </button>
                ))}
            </div>
          )}
        </>
      )}
      {(search.trim() || (!focus && kind)) && (
        <section className={styles.results} aria-label="Object search results">
          {searchQuery.isError ? (
            <ReadState
              error={searchQuery.error}
              retry={() => void searchQuery.refetch()}
              subject="matching objects"
            />
          ) : !searchQuery.data ? (
            <p role="status">Finding objects…</p>
          ) : (
            <>
              {searchQuery.data.results.filter(n => !missing.has(n.id)).map((n) => (
                <button
                  className={styles.result}
                  key={n.id}
                  onClick={() => {
                    remember(n);
                    update({ focus: n.id, selected: n.id, q: "", offset: "0" });
                  }}
                >
                  <strong>{n.name}</strong>
                  <span>{humanLabel(n.kind)}</span>
                </button>
              ))}
              {!searchQuery.data.results.length && (
                <p>
                  No objects match this search in this space. Try another name.
                </p>
              )}
              <div className={styles.spread}>
                <span>
                  {searchQuery.data.results.length} results on this page
                </span>
                <div>
                  <button
                    className="btn btn-sm"
                    disabled={offset === 0}
                    onClick={() =>
                      update({ offset: String(Math.max(0, offset - 20)) })
                    }
                  >
                    Previous results
                  </button>
                  <button
                    className="btn btn-sm"
                    disabled={
                      searchQuery.data.has_more === false ||
                      searchQuery.data.results.length < 20
                    }
                    onClick={() => update({ offset: String(offset + 20) })}
                  >
                    Next results
                  </button>
                </div>
              </div>
            </>
          )}
        </section>
      )}
      {!focus ? (
        <div className={styles.start}>
          <svg viewBox="0 0 340 150" aria-hidden>
            <path
              d="M40 118 210 85 310 125 140 148Z"
              fill="var(--accent-soft)"
            />
            <path d="m60 80 65-12 34 20-65 12Z" fill="var(--kind-repository)" />
            <path d="m60 80 34 20v27L60 107Z" fill="var(--border-field)" />
            <path d="m94 100 65-12v27l-65 12Z" fill="var(--ink-dim)" />
            <path
              d="m160 112 85-17"
              fill="none"
              stroke="var(--ink-dim)"
              strokeDasharray="4 5"
            />
            <circle
              cx="255"
              cy="92"
              r="8"
              fill="var(--surface)"
              stroke="var(--ink-dim)"
            />
          </svg>
          <h3>Start with something you know.</h3>
          <p>
            Find a product, an order or a place. Its neighbourhood opens here,
            one hop at a time.
          </p>
          <p className={styles.help}>
            Only objects you can read in this memory space are included.
          </p>
        </div>
      ) : (
        <>
          <button className={`btn btn-sm ${styles.filterToggle}`} aria-expanded={filtersOpen} aria-controls="connection-filters" onClick={() => setFiltersOpen(v => !v)}>Filters</button>
          <div id="connection-filters" className={styles.filters} data-open={filtersOpen}>
            <label>
              Depth
              <select
                aria-label="Depth"
                value={depth}
                onChange={(e) => update({ depth: e.target.value })}
              >
                {[1, 2, 3].map((n) => (
                  <option key={n} value={n}>
                    {n} {n === 1 ? "hop" : "hops"}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Relation
              <select
                aria-label="Relation"
                value={relation}
                onChange={(e) => update({ relation: e.target.value })}
              >
                <option value="">All relations</option>
                {[
                  ...new Set([
                    ...(query.data?.edges.map((e) => e.label) ?? []),
                    ...(relation ? [relation] : []),
                  ]),
                ]
                  .sort()
                  .map((r) => (
                    <option key={r} value={r}>
                      {humanLabel(r)}
                    </option>
                  ))}
              </select>
            </label>
            <label>
              Direction
              <select
                aria-label="Direction"
                value={direction}
                onChange={(e) => update({ direction: e.target.value })}
              >
                <option value="both">In and out</option>
                <option value="in">Incoming to focus</option>
                <option value="out">Outgoing from focus</option>
              </select>
            </label>
            <button
              className={`btn btn-sm ${styles.viewToggle}`}
              aria-pressed={table}
              onClick={() => update({ view: table ? "canvas" : "table" })}
            >
              {table ? "Show canvas" : "Show as table"}
            </button>
          </div>
          {query.isError && !scene ? (
            <div className={styles.state}>
              <h3>
                {denied
                  ? "This object is unavailable here."
                  : "The connections could not be loaded."}
              </h3>
              <ReadState
                error={query.error}
                retry={() => void query.refetch()}
                subject="this neighbourhood"
              />
            </div>
          ) : !scene ? (
            <div className={styles.state} role="status">
              <h3>Opening this neighbourhood…</h3>
              <p>Reading the objects and links you can access in this space.</p>
            </div>
          ) : (
            <>
              {query.isError && <ReadState error={query.error} retry={() => void query.refetch()} subject="this neighbourhood" />}
              <div className={styles.summary} role="status">
                {nodes.get(focus)?.name ?? "Object unavailable"} ·{" "}
                {scene.nodes.length} {plural("object", scene.nodes.length)} ·{" "}
                {scene.edges.length} {plural("link", scene.edges.length)} ·
                within {depth} {depth === 1 ? "hop" : "hops"}
              </div>
              {(query.data?.truncated || query.data?.partial) && (
                <p className={styles.limit}>
                  Partial neighbourhood. The service returns at most 150 objects
                  and 600 links, with a per-object limit. More may exist. Paging
                  is not available from this service yet. Filters and counts
                  describe only these loaded results.
                </p>
              )}
              {!scene.nodes.length ? (
                <div className={styles.state}>
                  <h3>No object returned.</h3>
                  <p>Search for another object in this space.</p>
                </div>
              ) : (
                <>
                  {!scene.edges.length && (
                    <p className={styles.limit}>
                      {query.data?.edges.length
                        ? "No links match these filters within this loaded neighbourhood."
                        : "This object is disconnected in this neighbourhood. No links were returned within the selected depth."}
                    </p>
                  )}
                  <div className={styles.workspace}>
                    <div className={styles.mainView}>
                      {!mobile && (
                        <div
                          className={
                            table ? styles.hidden : styles.desktopCanvas
                          }
                        >
                          <ConnectionCanvas
                            key={focus}
                            input={canvasInput}
                            saveInput={setCanvasInput}
                            scene={scene}
                            focus={focus}
                            selected={selected}
                            active={active}
                            activeKind={activeKind}
                            select={select}
                            hover={setHovered}
                          />
                        </div>
                      )}
                      {(table || mobile) && (
                        <div
                          className={table ? styles.tables : styles.mobileTable}
                        >
                          <p className={styles.mobileNote}>
                            Local neighbourhood as a table. Full-size names, the
                            same filters and counts.
                          </p>
                          <table className="data-table">
                            <caption>
                              Objects in this neighbourhood (
                              {scene.nodes.length})
                            </caption>
                            <thead>
                              <tr>
                                <th>Object</th>
                                <th>Kind</th>
                              </tr>
                            </thead>
                            <tbody>
                              {scene.nodes.map((n) => (
                                <tr
                                  key={n.id}
                                  data-active={active === n.id}
                                  onMouseEnter={() => setHovered(n.id)}
                                  onMouseLeave={() => setHovered("")}
                                >
                                  <th scope="row">
                                    <button
                                      className="link"
                                      data-object={n.id}
                                      aria-pressed={selected === n.id}
                                      onClick={() => select(n.id)}
                                    >
                                      {n.name}
                                    </button>
                                  </th>
                                  <td>{humanLabel(n.kind)}</td>
                                </tr>
                              ))}
                            </tbody>
                          </table>
                          <table className="data-table">
                            <caption>
                              Directed links ({scene.edges.length})
                            </caption>
                            <thead>
                              <tr>
                                <th>Recorded connection</th>
                              </tr>
                            </thead>
                            <tbody>
                              {scene.edges.map((e) => (
                                <tr key={e.id}>
                                  <td>{edgeSentence(e, nodes)}.</td>
                                </tr>
                              ))}
                            </tbody>
                          </table>
                        </div>
                      )}
                      <section
                        className={styles.reading}
                        aria-label="Connections in words"
                      >
                        <h3>
                          {selectedNode
                            ? `${selectedNode.name}: links in and out`
                            : "Read a connection"}
                        </h3>
                        <p className={styles.help}>
                          Arrows follow each stored relationship. A link does
                          not predict a consequence.
                        </p>
                        <ul>
                          {(selectedNode ? related : scene.edges).map((e) => (
                            <li
                              key={e.id}
                              data-active={active === e.from || active === e.to}
                              onMouseEnter={() => setHovered(e.from)}
                              onMouseLeave={() => setHovered("")}
                            >
                              <button
                                onFocus={() => setHovered(e.from)}
                                onBlur={() => setHovered("")}
                                onClick={() =>
                                  select(e.from === selected ? e.to : e.from)
                                }
                              >
                                {edgeSentence(e, nodes)}.
                              </button>
                            </li>
                          ))}
                        </ul>
                        {selectedNode && !related.length && (
                          <p>
                            No links for this selection match the current
                            filters.
                          </p>
                        )}
                      </section>
                    </div>
                    {selectedNode ? (
                      <ConnectionInspector
                        key={selectedNode.id}
                        space={space}
                        node={selectedNode}
                        close={close}
                      />
                    ) : (
                      <ConnectionSummary
                        scene={scene}
                        focus={focus}
                        highlight={setActiveKind}
                      />
                    )}
                  </div>
                </>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}
