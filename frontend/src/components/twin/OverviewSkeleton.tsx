import styles from "./overview.module.css";
export default function OverviewSkeleton() {
  return (
    <div
      role="status"
      aria-label="Loading business overview"
      className={styles.overview}
    >
      <span className="sr-only">Loading business overview</span>
      <div aria-hidden="true" className={styles.skeleton}>
        <div className={styles.bone} style={{ width: "65%", height: "2rem" }} />
        <div
          className={styles.bone}
          style={{ width: "42%", marginTop: "1rem" }}
        />
        <div className={styles.hero}>
          <div className={styles.map}>
            {[0, 1, 2].map((i) => (
              <div
                key={i}
                className={styles.bone}
                style={{ height: "4.5rem", margin: "1rem 0" }}
              />
            ))}
          </div>
          <div>
            {[0, 1, 2].map((i) => (
              <div
                key={i}
                className={styles.bone}
                style={{ height: "5rem", margin: "1.5rem 0" }}
              />
            ))}
          </div>
        </div>
        <div className={styles.bone} style={{ height: "6rem" }} />
        <div
          className={styles.bone}
          style={{ height: "8rem", marginTop: "2rem" }}
        />
      </div>
    </div>
  );
}
