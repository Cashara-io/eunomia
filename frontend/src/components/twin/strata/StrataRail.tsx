"use client";
import Link from "next/link";
import { useKindColor } from "@/lib/kindColors";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { ConnectionNode } from "@/lib/queries/connections";
import { twinHref } from "../model";
import { humanLabel, plural } from "../overview-format";
import { kindRows } from "./strata-model";
import styles from "./strata.module.css";

export default function StrataRail({ data, level, kind, focusKind, confirmed, select, space, waiting, recent, pick }: {
  recent: ConnectionNode[];
  pick: (node: ConnectionNode) => void;
  data?: OntologyOut;
  level: "kinds" | "objects";
  kind: string;
  focusKind?: string;
  confirmed: boolean;
  select: (kind: string) => void;
  space: string;
  waiting: number;
}) {
  const color = useKindColor();
  const rows = confirmed && data ? kindRows(data) : null;
  const current = level === "kinds" ? kind : (focusKind ?? "");
  return (
    <nav className={styles.rail} aria-label="Kinds">
      <h2>Kinds, by object count</h2>
      {rows && (
        <Link className={styles.reviewEntry} href={twinHref("/ontology/review", space)}>
          <span>Review</span>
          {waiting > 0 && (
            <span className={styles.pending} aria-label={`${waiting} ${plural("definition", waiting)} in review`}>
              {waiting}
            </span>
          )}
        </Link>
      )}
      <ul>
        {rows
          ? rows.map((row) => (
              <li key={row.kind}>
                <button aria-current={current === row.kind ? "true" : undefined} onClick={() => select(row.kind)}>
                  <span className={styles.swatch} style={{ background: color(row.kind) }} aria-hidden />
                  <span className={styles.railName}>{humanLabel(row.kind)}</span>
                  {row.waiting > 0 && (
                    <span className={styles.pending} title={`${row.waiting} definition waits for review`} aria-label={`${row.waiting} in review`}>
                      {row.waiting}
                    </span>
                  )}
                  <span className="font-mono">{row.entities.toLocaleString("en-AU")}</span>
                </button>
              </li>
            ))
          : Array.from({ length: 6 }, (_, i) => (
              <li key={i} className={styles.bone} aria-hidden />
            ))}
      </ul>
      {rows && level === "objects" && recent.length > 0 && (
        <div className={styles.recent} aria-label="Recently viewed objects">
          <h2>Recently viewed</h2>
          {recent.map((n) => (
            <button key={n.id} onClick={() => pick(n)}>
              <span className={styles.railName}>{n.name}</span>
            </button>
          ))}
        </div>
      )}
    </nav>
  );
}
