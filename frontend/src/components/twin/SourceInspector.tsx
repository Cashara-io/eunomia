import { useEffect, useRef, useState, type Ref } from "react";
import Link from "next/link";
import type { ObjectProfile } from "@/lib/queries/objects";
import { formatted } from "./value-format";
import { humanLabel } from "./overview-format";
import { sourceFor, sourceAvailable, sourceLabel, sourceMismatch, caseOnlyNote, sourceColumn, rowUnavailable } from "./receipt-model";
import styles from "./receipts.module.css";

/** The exact source row, or record, behind one value. Shared by the profile inspector and the Strata stage. */
export function SourceReceiptBody({ data, name, space, close, heading }: { data: ObjectProfile; name: string; space: string; close: () => void; heading: Ref<HTMLHeadingElement> }) {
  const property = data.properties.find(p => p.name === name);
  const source = property && sourceFor(data, property);
  const allowed = sourceAvailable(source) && property?.state !== "unavailable" && property?.source_state !== "unavailable";
  const mismatch = property ? sourceMismatch(property, source) : null;
  const caseOnly = mismatch === caseOnlyNote;
  return <>
    <div className={styles.inspectorHead}><span className={styles.plate}>Source receipt</span><button className="btn btn-sm btn-ghost" onClick={close}>Close source</button></div>
    <p className={styles.context}>{data.name}</p>
    <h3 ref={heading} tabIndex={-1}>{property ? humanLabel(property.name) : "Value unavailable"}</h3>
    {property && <p className={styles.inspectedValue}>{property.state === "unavailable" ? "Unavailable" : formatted(property.value, property)}</p>}
    <div className={styles.sourceTitle}>{allowed ? sourceLabel(source) : "Source unavailable"}</div>
    {!allowed ? <p role="status">This source is unavailable in this space. The document may have been deleted, removed from this space or access may be restricted.</p> : source?.document ? <>
      <p className={styles.dim}>Current file. Original row and revision were not retained.</p>
      {source.document.row ? <dl className={styles.cells}>{Object.entries(source.document.row.cells).map(([column, value]) => <div key={column} data-highlight={sourceColumn(column) === name} data-mismatch={sourceColumn(column) === name && !!mismatch && !caseOnly}><dt>{humanLabel(column)}</dt><dd>{value || "Missing"}</dd>{sourceColumn(column) === name && mismatch && <dd className={caseOnly ? styles.caseNote : styles.mismatch} role="note">{mismatch}</dd>}</div>)}</dl> : <p role="status">{rowUnavailable}</p>}
      <Link className="link" href={`/documents?id=${encodeURIComponent(source.document.id)}&space=${encodeURIComponent(space)}`}>Open document</Link>
    </> : source?.record ? <><p>{source.record.source} · Exact source record</p><pre className={styles.payload}>{JSON.stringify(source.record.payload, null, 2)}</pre></> : <p>The source record was not retained.</p>}
  </>;
}

export default function SourceInspector({ data, name, space, close, restoreFocus }: { data: ObjectProfile; name: string; space: string; close: () => void; restoreFocus: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const restore = useRef(restoreFocus);
  useEffect(() => { restore.current = restoreFocus; }, [restoreFocus]);
  const [mobile, setMobile] = useState(false);
  useEffect(() => {
    const media = window.matchMedia("(max-width: 700px)");
    const change = () => setMobile(media.matches);
    change();
    media.addEventListener("change", change);
    return () => media.removeEventListener("change", change);
  }, []);
  useEffect(() => {
    if (mobile) dialog.current?.showModal();
    heading.current?.focus();
    return () => restore.current();
  }, [mobile, name]);
  const contents = <SourceReceiptBody data={data} name={name} space={space} close={close} heading={heading} />;
  if (mobile) return <dialog ref={dialog} className={styles.sheet} aria-label="Source inspector" onCancel={e => { e.preventDefault(); close(); }}>{contents}</dialog>;
  return <aside className={styles.inspector} aria-label="Source inspector" onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); close(); } }}>{contents}</aside>;
}
