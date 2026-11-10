import type { Definition } from "./model";
import { reviewFields } from "./review-model";
import { comparisonRows } from "./review-visual-model";
import styles from "./review.module.css";

export default function ReviewComparison({ before, after }: { before?: Definition; after: Definition }) {
  if (!before) return <>
    <h5 className={styles.proposalTitle}>New definition</h5>
    <p className={styles.baselineNotice}>There is nothing to compare against yet.</p>
    <ul className={styles.proposal} aria-label="New definition">
      {Object.entries(reviewFields(after))
        .filter(([field, value]) => !["Suggested reason", "Source mapping", "Samples", "Merge destination"].includes(field) && value !== "None")
        .map(([field, value]) => <li key={field}><strong>{field === "Aliases" ? "Also called" : field}</strong>{" "}<span>{value}</span></li>)}
    </ul>
  </>;
  return <div className={styles.comparison} role="table" aria-label="Definition comparison">
    <div className={styles.comparisonHead} role="row"><span role="columnheader">Field</span><span role="columnheader">Current</span><span role="columnheader">Proposed</span></div>
    {comparisonRows(before, after).filter(row => !["Suggested reason", "Source mapping", "Samples", "Merge destination"].includes(row.field)).filter(row => row.proposed !== "None" || row.previous && row.previous !== "None").map(row => <div role="row" className={styles.comparisonRow} data-change={row.change} key={row.field}>
      <div role="rowheader"><strong>{row.field === "Aliases" ? "Also called" : row.field}</strong><span className={styles.changeMark}>{row.change === "Unavailable" ? "Not comparable" : row.change}</span></div>
      <span role="cell" className={styles.dim}>{row.change === "Removed" ? <del>{row.previous}</del> : row.previous ?? "Not supplied"}</span>
      <span role="cell">{row.change === "Added" ? <ins>{row.proposed}</ins> : row.proposed}</span>
    </div>)}
  </div>;
}
