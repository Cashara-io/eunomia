"use client";
import { useState } from "react";
import Link from "next/link";
import { ArrowRightIcon, GraphIcon, TableIcon } from "@phosphor-icons/react";
import type { OntologyOut } from "@/lib/queries/ontology";
import { words } from "./DefinitionRows";
import { twinHref } from "./model";
import { businessSummary } from "./overview-model";
import styles from "./overview.module.css";

export default function KindSummary({
  data,
  space,
}: {
  data: OntologyOut;
  space: string;
}) {
  const { top, kinds, links } = businessSummary(data);
  const [table, setTable] = useState(false);
  const [selected, setSelected] = useState("");
  const focus = top.find((k) => k.kind === selected);
  const linked = new Set(links.flatMap((l) => [l.from_kind, l.to_kind]));
  function node(name: string) {
    const kind = top.find((k) => k.kind === name);
    return (
      <button
        className={styles.node}
        aria-pressed={selected === name}
        onClick={() => setSelected(selected === name ? "" : name)}
      >
        <span>{words(name)}</span>
        <span className={styles.nodeCount}>
          {kind?.entities.toLocaleString("en-AU")} objects
        </span>
      </button>
    );
  }
  return (
    <section className={styles.map} aria-labelledby="kind-map-title">
      <div className={styles.sectionHead}>
        <h2 id="kind-map-title">How it fits together</h2>
        <button
          className="btn btn-ghost btn-sm"
          aria-pressed={table}
          onClick={() => setTable(!table)}
        >
          {table ? (
            <GraphIcon aria-hidden size={16} />
          ) : (
            <TableIcon aria-hidden size={16} />
          )}
          {table ? "Show diagram" : "Show table"}
        </button>
      </div>
      <p className={styles.caption}>
        Kinds, not individual objects. Strongest recorded links among the
        largest kinds.
      </p>
      {table ? (
        <div
          className={styles.tableRegion}
          role="region"
          aria-label="Kind connections table"
          tabIndex={0}
        >
          <table className="data-table w-full">
            <caption className="sr-only">
              The same connections as the diagram
            </caption>
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
                <tr key={JSON.stringify([l.from_kind, l.label, l.to_kind])}>
                  <td>{words(l.from_kind)}</td>
                  <td>{words(l.label)}</td>
                  <td>{words(l.to_kind)}</td>
                  <td className="font-mono">
                    {l.count.toLocaleString("en-AU")}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {links.length === 0 && (
            <p>No recorded links between these kinds yet.</p>
          )}
        </div>
      ) : (
        <div className={styles.diagram} aria-label="Kind connection diagram">
          {links.map((l) => (
            <div
              className={styles.connection}
              key={JSON.stringify([l.from_kind, l.label, l.to_kind])}
            >
              {node(l.from_kind)}
              <div className={styles.edge}>
                <span>{words(l.label)}</span>
                <svg
                  viewBox="0 0 100 12"
                  preserveAspectRatio="none"
                  aria-hidden="true"
                >
                  <path d="M0 6H96M90 1l7 5-7 5" />
                </svg>
                <span className={styles.edgeCount}>
                  {l.count.toLocaleString("en-AU")} links
                </span>
              </div>
              {node(l.to_kind)}
            </div>
          ))}
          <div className={styles.unlinked}>
            {top
              .filter((k) => !linked.has(k.kind))
              .map((k) => (
                <div key={k.kind}>{node(k.kind)}</div>
              ))}
          </div>
          {links.length === 0 && (
            <p className={styles.caption}>
              No recorded links between these kinds yet. Kinds can still have
              objects.
            </p>
          )}
        </div>
      )}
      <div className={styles.explain}>
        {focus ? (
          <>
            <span className={styles.caption}>
              {words(focus.kind)}
              {focus.samples.length
                ? ` includes ${focus.samples.slice(0, 2).join(", ")}.`
                : " has no named samples yet."}{" "}
              Definition:{" "}
              {focus.status === "observed"
                ? "used, not reviewed"
                : focus.status}
              .
            </span>
            <Link
              className="link"
              href={twinHref("/ontology/objects", space, { kind: focus.kind })}
            >
              Find {words(focus.kind)} <ArrowRightIcon aria-hidden size={14} />
            </Link>
          </>
        ) : (
          <span className={styles.caption}>
            Select a kind to see its examples and open its objects.
          </span>
        )}
      </div>
      <footer className={styles.mapFoot}>
        <span>
          {top.length} of {kinds.length} kinds · {links.length} of{" "}
          {
            data.links.filter((l) => l.count > 0 && l.status !== "rejected")
              .length
          }{" "}
          recorded link types
        </span>
        <Link className="link" href={twinHref("/ontology/schema", space)}>
          All structure <ArrowRightIcon aria-hidden size={14} />
        </Link>
      </footer>
    </section>
  );
}
