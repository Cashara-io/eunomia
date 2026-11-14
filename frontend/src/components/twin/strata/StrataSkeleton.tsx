import styles from "./strata.module.css";

/** The stage as placeholders: shapes only, no data. */
export default function StrataSkeleton({ objects }: { objects: boolean }) {
  return (
    <div className={styles.skeleton} aria-busy="true">
      <p role="status" className={styles.planeNote}>
        {objects ? "Opening this neighbourhood. Reading the objects and links you can access in this space." : "Reading the kinds in this space."}
      </p>
      <div className={styles.skeletonGrid} aria-hidden>
        {Array.from({ length: objects ? 7 : 10 }, (_, i) => (
          <div key={i} className={styles.bone} data-center={objects && i === 3} />
        ))}
      </div>
    </div>
  );
}
