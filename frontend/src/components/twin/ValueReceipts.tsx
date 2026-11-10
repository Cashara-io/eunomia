import type { ObjectProfile } from "@/lib/queries/objects";
import { formatted } from "./value-format";
import { humanLabel } from "./overview-format";
import { ReviewTime } from "./review-format";
import { receiptState, sourceFor, sourceLabel, sourceMismatch, caseOnlyNote } from "./receipt-model";
import styles from "./receipts.module.css";
export default function ValueReceipts({ data, inspect, selected }: { data: ObjectProfile; inspect: (name: string, button: HTMLButtonElement) => void; selected: string | null }) {
  return <dl className={styles.receipts}>
    {data.properties.length === 0 && <div className={styles.empty}><dt>No approved properties</dt><dd>Values and their sources will appear here when they are recorded.</dd></div>}
    {data.properties.map(p => {
      const mismatch = sourceMismatch(p, sourceFor(data, p));
      const caseOnly = mismatch === caseOnlyNote;
      return <div key={p.name} className={styles.receipt} data-selected={p.name === selected} data-mismatch={!!mismatch && !caseOnly}>
      <dt>{humanLabel(p.name)}</dt>
      <dd className={styles.value}>{p.state === "unavailable" ? "Unavailable" : formatted(p.value, p)}</dd>
      <dd className={styles.state} data-state={p.state}>{receiptState(p)}{p.unit && <span> · Unit: {p.unit}</span>}</dd>
      {mismatch ? <dd className={caseOnly ? styles.caseNote : styles.mismatch} role="note">{mismatch}</dd> : ["stale", "missing", "conflicting", "unavailable"].includes(p.state) && <dd className={styles.explanation}>{p.state === "stale" ? "The source changed after this value was recorded." : p.state === "missing" ? "No value has been recorded." : p.state === "conflicting" ? "Different values share the latest as-of time." : "The source cannot be read in this space."}</dd>}
      <dd className={styles.sourceLine}><button className="link" aria-label={`Inspect source for ${humanLabel(p.name)}`} onClick={e => inspect(p.name, e.currentTarget)}>{!p.source ? "Source not recorded" : p.source_state === "unavailable" ? "Source unavailable" : sourceLabel(sourceFor(data, p))}</button><span>{p.as_of && p.as_of !== "NONE" ? <>As of <ReviewTime at={p.as_of} /></> : "Date not recorded"}{p.recorded_at && <> · Recorded <ReviewTime at={p.recorded_at} /></>}</span></dd>
    </div>; })}
  </dl>;
}
