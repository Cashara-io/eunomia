import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { useObjectProfile } from "@/lib/queries/objects";
import type { ConnectionNode } from "@/lib/queries/connections";
import { formatted, objectHref } from "./ObjectProfile";
import { humanLabel, relativeTime } from "./overview-format";
import ReadState from "./ReadState";
import styles from "./connections.module.css";
import { sourceAvailable, sourceLabel, sourceMismatch } from "./receipt-model";
import receiptStyles from "./receipts.module.css";
import shared from "./overview.module.css";
export default function ConnectionInspector({
  space,
  node,
  close,
}: {
  space: string;
  node: ConnectionNode;
  close: () => void;
}) {
  const [now] = useState(() => Date.now());
  const panel = useRef<HTMLElement>(null);
  useEffect(() => {
    if (window.matchMedia("(max-width: 640px)").matches) panel.current?.focus();
  }, [node.id]);
  const query = useObjectProfile(space, node.id);
  return (
    <aside
      className={styles.inspector}
      aria-label="Selected object"
      ref={panel}
      tabIndex={-1}
    >
      <div className={styles.spread}>
        <span className="label">{humanLabel(node.kind)} · Selected</span>
        <button className="btn btn-sm btn-ghost" onClick={close}>
          Close inspector
        </button>
      </div>
      <h3>{node.name}</h3>
      {query.isError ? (
        <ReadState
          error={query.error}
          retry={() => void query.refetch()}
          subject="this object"
        />
      ) : !query.data ? (
        <p role="status">Loading values and sources…</p>
      ) : (
        <>
          <p>{query.data.summary || "No summary recorded."}</p>
          <h4>Values and their sources</h4>
          <dl>
            {query.data.properties.map((p) => {
              const source = query.data.sources.find((s) => s.id === p.source);
              const mismatch = sourceMismatch(p, source);
              return (
                <div className={styles.value} key={p.name}>
                  <dt>{humanLabel(p.name)}</dt>
                  <dd className="font-mono">
                    {p.state === "unavailable"
                      ? "Unavailable"
                      : formatted(p.value, p)}
                  </dd>
                  <dd className={styles.receipt}>
                    <span
                      className={
                        p.state === "stale" ? shared.attentionPin : undefined
                      }
                    >
                      {humanLabel(p.state)}
                    </span>
                    {p.unit ? ` · ${p.unit}` : ""}
                    {p.as_of && p.as_of !== "NONE"
                      ? ` · ${relativeTime(p.as_of, now)}`
                      : " · Date not recorded"}
                    <br />
                    {p.source ? p.source_state === "unavailable" ? "Source unavailable" : sourceLabel(source) : "Source not recorded"}
                    {sourceAvailable(source) && source?.record && (
                      <span> · {source.record.external_id}</span>
                    )}
                    {mismatch && <p role="note" className={receiptStyles.mismatch}>{mismatch}</p>}
                    {p.state === "stale" && !mismatch && (
                      <p>The source changed after this value was recorded.</p>
                    )}
                    {p.state === "missing" && (
                      <p>No value has been recorded.</p>
                    )}
                  </dd>
                </div>
              );
            })}
          </dl>
          {query.data.properties.length === 0 && (
            <p>No approved properties recorded.</p>
          )}
          <Link
            className="link"
            href={`${objectHref(node.id, space)}&section=Sources`}
          >
            Open full source records
          </Link>
        </>
      )}
    </aside>
  );
}
