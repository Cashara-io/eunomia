"use client";
import Link from "next/link";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { ConnectionNode } from "@/lib/queries/connections";
import { edgeSentence, type Scene } from "../connections-model";
import { kindHref } from "../model";
import { humanLabel, linkSentence, plural } from "../overview-format";
import { kindRows } from "./strata-model";
import styles from "./strata.module.css";

export function KindsTables({ data, space, select }: { data: OntologyOut; space: string; select: (kind: string) => void }) {
  const rows = kindRows(data);
  const links = data.links.filter((l) => l.status !== "rejected");
  return (
    <div className={styles.tables}>
      <p className={styles.planeNote}>Kinds as a table. Full names, the same counts.</p>
      <table className="data-table">
        <caption>Kinds in this space ({rows.length})</caption>
        <thead>
          <tr>
            <th>Kind</th>
            <th>Objects</th>
            <th>In review</th>
            <th>Definition</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.kind}>
              <th scope="row">
                <button className="link" onClick={() => select(r.kind)}>
                  {humanLabel(r.kind)}
                </button>
              </th>
              <td className="font-mono">{r.entities.toLocaleString("en-AU")}</td>
              <td className="font-mono">{r.waiting}</td>
              <td>
                <Link className="link" href={kindHref(space, r.kind)}>
                  Read {humanLabel(r.kind)}
                </Link>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <table className="data-table">
        <caption>Recorded link types ({links.length})</caption>
        <thead>
          <tr>
            <th>Link</th>
            <th>Recorded</th>
          </tr>
        </thead>
        <tbody>
          {links.map((l) => (
            <tr key={`${l.from_kind}/${l.label}/${l.to_kind}`}>
              <td>{linkSentence(l)}</td>
              <td className="font-mono">{l.count.toLocaleString("en-AU")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function ObjectsTables({ scene, nodes, selected, select }: {
  scene: Scene;
  nodes: ReadonlyMap<string, ConnectionNode>;
  selected: string;
  select: (id: string) => void;
}) {
  return (
    <div className={styles.tables}>
      <p className={styles.planeNote}>Local neighbourhood as a table. Full-size names, the same filters and counts.</p>
      <table className="data-table">
        <caption>
          Objects in this neighbourhood ({scene.nodes.length} {plural("object", scene.nodes.length)})
        </caption>
        <thead>
          <tr>
            <th>Object</th>
            <th>Kind</th>
          </tr>
        </thead>
        <tbody>
          {scene.nodes.map((n) => (
            <tr key={n.id}>
              <th scope="row">
                <button className="link" data-object={n.id} aria-pressed={selected === n.id} onClick={() => select(n.id)}>
                  {n.name}
                </button>
              </th>
              <td>{humanLabel(n.kind)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <table className="data-table">
        <caption>Directed links ({scene.edges.length})</caption>
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
  );
}
