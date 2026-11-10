import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import styles from "./review.module.css";

export default function ReviewSkeleton() {
  return <LoadingSkeleton label="Loading the ontology">
    <div className={styles.workspace}>
      <header className={styles.heading}><h2 className={styles.title}>Review suggestions</h2></header>
      <div className={styles.layout}>
        <div className={styles.queue}><div className={styles.queuehead}>Decision inbox</div>
          {[0, 1, 2].map(i => <div key={i} className={styles.queuelink}><p>Suggested definition</p><p>Kind and proposed change</p><p>Proposer and time</p></div>)}
        </div>
        <div className={styles.detail}>
          <header className={styles.identity}><h3>Definition under review</h3><p>Proposed definition and revision</p></header>
          <div className={styles.section}><h4>What changes</h4><p>Current and proposed fields</p></div>
          <div className={styles.contextPicture}><svg viewBox="0 0 600 140" className={styles.contextPlane} aria-hidden="true"><path d="M50 120L550 80 500 20 0 60Z" fill="var(--border)" /></svg></div>
          <div className={styles.section}><h4>Read the source</h4><div className={styles.source}>Source file and column</div></div>
        </div>
      </div>
    </div>
  </LoadingSkeleton>;
}
