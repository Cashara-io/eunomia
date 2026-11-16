"use client";
import { useRef, useState } from "react";
import Link from "next/link";
import { ArrowRightIcon } from "@phosphor-icons/react";
import { useKindColor } from "@/lib/kindColors";
import type { OntologyOut } from "@/lib/queries/ontology";
import { useObjectProfile } from "@/lib/queries/objects";
import type { ConnectionNode } from "@/lib/queries/connections";
import type { Vault } from "@/lib/types";
import { definitions, kindHref, twinHref } from "../model";
import { formatted, objectHref } from "../ObjectProfile";
import { humanLabel, linkSentence, plural, relativeTime } from "../overview-format";
import ReadState from "../ReadState";
import { receiptState, sourceFor, sourceLabel } from "../receipt-model";
import StructureCounts from "./StructureCounts";
import StrataReceipt from "./StrataReceipt";
import { kindRows, type Lane } from "./strata-model";
import styles from "./strata.module.css";

export type Crumb = { label: string; onClick?: () => void };

function Crumbs({ items }: { items: Crumb[] }) {
  return (
    <nav className={styles.crumbs} aria-label="Breadcrumb">
      {items.map((c, i) => (
        <span key={i}>
          {i > 0 && <span aria-hidden> / </span>}
          {c.onClick ? <button onClick={c.onClick}>{c.label}</button> : <span aria-current="location">{c.label}</span>}
        </span>
      ))}
    </nav>
  );
}

export function KindInspector({ vault, data, kind, selectKind, openObjects }: {
  vault: Vault;
  data: OntologyOut;
  kind: string;
  selectKind: (kind: string) => void;
  openObjects: (kind: string) => void;
}) {
  const color = useKindColor();
  const rows = kindRows(data);
  const waiting = definitions(data).filter((d) => d.item.status === "proposed");
  const total = rows.reduce((n, r) => n + r.entities, 0);
  const item = data.kinds.find((k) => k.kind === kind);
  if (!item)
    return (
      <>
        <Crumbs items={[{ label: vault.name }]} />
        <h3>
          {rows.length.toLocaleString("en-AU")} {plural("kind", rows.length)}
        </h3>
        <p>
          <span className="font-mono">{total.toLocaleString("en-AU")}</span> {plural("object", total)} in {vault.name}.{" "}
          {waiting.length
            ? `${waiting.length} ${plural("definition", waiting.length)} ${waiting.length === 1 ? "waits" : "wait"} for your judgement.`
            : "Nothing is waiting for review."}
        </p>
        {waiting.length > 0 && (
          <>
            <h4 className={styles.section}>Needs your judgement</h4>
            <ul className={styles.rows}>
              {waiting.slice(0, 5).map((d) => (
                <li key={d.id}>
                  <Link className={styles.row} href={twinHref("/ontology/review", vault.id, { selected: d.id })}>
                    <span>{humanLabel(d.name)}</span>
                    <small>
                      Suggested {d.type}
                      {d.type === "link" ? "" : ` on ${humanLabel(d.kind)}`}. Read the evidence.
                    </small>
                  </Link>
                </li>
              ))}
            </ul>
            <Link className="btn btn-sm" href={twinHref("/ontology/review", vault.id)}>
              Review {waiting.length} {plural("suggestion", waiting.length)} <ArrowRightIcon size={14} aria-hidden />
            </Link>
          </>
        )}
        <p className={styles.hint}>Select a kind to read its definition and its links.</p>
        <StructureCounts vaultId={vault.id} data={data} />
      </>
    );
  const links = data.links.filter((l) => l.status !== "rejected" && (l.from_kind === kind || l.to_kind === kind));
  const mine = waiting.filter((d) => d.kind === kind);
  return (
    <>
      <Crumbs items={[{ label: vault.name, onClick: () => selectKind("") }, { label: humanLabel(kind) }]} />
      <h3>
        <span className={styles.swatch} style={{ background: color(kind) }} aria-hidden /> {humanLabel(kind)}
      </h3>
      <p>
        <span className="font-mono">{item.entities.toLocaleString("en-AU")}</span> {plural("object", item.entities)}.{" "}
        {item.description || "No description recorded."}
      </p>
      {mine.length > 0 && (
        <ul className={styles.rows}>
          {mine.map((d) => (
            <li key={d.id}>
              <Link className={styles.row} href={twinHref("/ontology/review", vault.id, { selected: d.id })}>
                <span>In review: {humanLabel(d.name)}</span>
                <small>Read the evidence to decide.</small>
              </Link>
            </li>
          ))}
        </ul>
      )}
      <h4 className={styles.section}>Recorded links</h4>
      <ul className={styles.rows}>
        {links.map((l) => {
          const other = l.from_kind === kind ? l.to_kind : l.from_kind;
          return (
            <li key={`${l.from_kind}/${l.label}/${l.to_kind}`}>
              <button className={styles.row} onClick={() => selectKind(other)}>
                <span>{linkSentence(l)}</span>
                <span className="font-mono">{l.count.toLocaleString("en-AU")}</span>
                {l.status === "proposed" && <small>Suggested, in review</small>}
              </button>
            </li>
          );
        })}
        {links.length === 0 && <li className={styles.hint}>No links recorded for this kind.</li>}
      </ul>
      <div className={styles.actions}>
        <button className="btn btn-primary btn-sm" onClick={() => openObjects(kind)} disabled={item.entities === 0}>
          Open {humanLabel(kind).toLowerCase()} as objects
        </button>
        <Link className="btn btn-sm" href={kindHref(vault.id, kind)}>
          Read the definition
        </Link>
      </div>
    </>
  );
}

export function ObjectInspector({ space, node, focus, focusName, value, selectValue, selectNode, makeFocus, close, mobile }: {
  space: string;
  node: ConnectionNode;
  focus: string;
  focusName: string;
  value: string;
  selectValue: (name: string) => void;
  selectNode: (id: string) => void;
  makeFocus: () => void;
  close: () => void;
  mobile: boolean;
}) {
  const color = useKindColor();
  const [now] = useState(() => Date.now());
  const query = useObjectProfile(space, node.id);
  const opener = useRef<HTMLButtonElement | null>(null);
  const data = query.data;
  const crumbs: Crumb[] = [
    { label: focusName || "Focus", onClick: node.id === focus && !value ? undefined : () => selectNode(focus) },
    ...(node.id !== focus ? [{ label: node.name, onClick: value ? () => selectValue("") : undefined }] : []),
    ...(value ? [{ label: humanLabel(value) }] : []),
  ];
  return (
    <>
      <Crumbs items={crumbs} />
      <h3>
        <span className={styles.swatch} style={{ background: color(node.kind) }} aria-hidden /> {node.name}
      </h3>
      <p className={styles.caption}>{humanLabel(node.kind)}, selected</p>
      {query.isError ? (
        <ReadState error={query.error} retry={() => void query.refetch()} subject="this object" />
      ) : !data ? (
        <p role="status">Loading values and sources…</p>
      ) : (
        <>
          <p>{data.summary || "No summary recorded."}</p>
          <h4 className={styles.section}>Values and their sources</h4>
          <ul className={styles.rows}>
            {data.properties.map((p) => {
              const source = sourceFor(data, p);
              const open = value === p.name;
              return (
                <li key={p.name}>
                  <button
                    className={styles.row}
                    data-wire={`value:${p.name}`}
                    data-state={p.state}
                    aria-pressed={open}
                    onClick={(e) => {
                      opener.current = e.currentTarget;
                      selectValue(open ? "" : p.name);
                    }}
                  >
                    <span>{humanLabel(p.name)}</span>
                    <span className="font-mono">{p.state === "unavailable" ? "Unavailable" : formatted(p.value, p)}</span>
                    <small>
                      {receiptState(p)}
                      {p.unit ? ` · ${p.unit}` : ""}
                      {p.as_of && p.as_of !== "NONE" ? ` · ${relativeTime(p.as_of, now)}` : " · Date not recorded"}
                      {". "}
                      {!p.source ? "Source not recorded" : p.source_state === "unavailable" ? "Source unavailable" : sourceLabel(source)}
                    </small>
                  </button>
                  {open && mobile && (
                    <StrataReceipt
                      inline
                      space={space}
                      id={node.id}
                      name={p.name}
                      close={() => {
                        selectValue("");
                        queueMicrotask(() => opener.current?.focus());
                      }}
                    />
                  )}
                </li>
              );
            })}
            {data.properties.length === 0 && <li className={styles.hint}>No approved properties recorded.</li>}
          </ul>
        </>
      )}
      <div className={styles.actions}>
        {node.id !== focus && (
          <button className="btn btn-sm" onClick={makeFocus}>
            Make this the focus
          </button>
        )}
        <Link className="btn btn-sm" href={objectHref(node.id, space)}>
          Open full profile
        </Link>
        <Link className="btn btn-sm" href={`${objectHref(node.id, space)}&section=Sources`}>
          Open full source records
        </Link>
        <button className="btn btn-sm btn-ghost" onClick={close}>
          Close inspector
        </button>
      </div>
    </>
  );
}

export function AroundInspector({ focusName, lanes, nodes, edges, partial, select, pickLane }: {
  focusName: string;
  lanes: Lane[];
  nodes: number;
  edges: number;
  partial: boolean;
  select: (id: string) => void;
  pickLane: (laneId: string) => void;
}) {
  const near = lanes.filter((l) => l.hop === 1);
  return (
    <>
      <Crumbs items={[{ label: focusName }]} />
      <h3>
        {Math.max(0, nodes - 1).toLocaleString("en-AU")} {plural("neighbour", Math.max(0, nodes - 1))}
      </h3>
      <p>
        {new Set(lanes.map((l) => l.kind)).size} {plural("kind", new Set(lanes.map((l) => l.kind)).size)},{" "}
        {edges.toLocaleString("en-AU")} recorded {plural("link", edges)}.{partial ? " Partial: more may exist." : ""} Select an object to read its values and where each one came from.
      </p>
      <ul className={styles.rows}>
        {near.map((l) => (
          <li key={l.id}>
            <button className={styles.row} onClick={() => (l.nodes.length === 1 ? select(l.nodes[0].id) : pickLane(l.id))}>
              <span>{humanLabel(l.kind)}</span>
              <span className="font-mono">{l.nodes.length.toLocaleString("en-AU")}</span>
              <small>{l.dir === "in" ? `${l.verb} ${focusName}` : `${focusName} ${l.verb}`}</small>
            </button>
          </li>
        ))}
      </ul>
    </>
  );
}
