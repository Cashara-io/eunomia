import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import styles from "./overview.module.css";
export default function OverviewSkeleton() {
  return (
    <LoadingSkeleton label="Loading business overview">
      <div aria-hidden="true" className={styles.skeleton}>
        <div
          skullet-block=""
          className={styles.bone}
          style={{ width: "60%", height: "2rem" }}
        />
        <div
          skullet-block=""
          className={styles.bone}
          style={{ width: "40%", marginTop: "1rem" }}
        />
        <div className={styles.map} style={{ marginTop: "2rem" }}>
          <div
            skullet-block=""
            className={styles.bone}
            style={{ width: "35%", height: "1.5rem" }}
          />
          <div className={styles.skeletonPlane}>
            {Array.from({ length: 16 }, (_, i) => (
              <div key={i}>
                <div
                  skullet-block=""
                  className={styles.bone}
                  style={{
                    width: "70%",
                    height: ".75rem",
                    margin: "0 auto .75rem",
                  }}
                />
                <div
                  skullet-block=""
                  className={styles.bone}
                  style={{ height: `${2 + (i % 3)}rem` }}
                />
              </div>
            ))}
          </div>
          <div className={styles.linkedPanels}>
            {[0, 1].map((i) => (
              <div key={i}>
                {[0, 1, 2, 3].map((j) => (
                  <div
                    key={j}
                    skullet-block=""
                    className={styles.bone}
                    style={{ height: "1.5rem", margin: ".75rem 0" }}
                  />
                ))}
              </div>
            ))}
          </div>
        </div>
        <div
          skullet-block=""
          className={styles.bone}
          style={{ height: "4rem", marginTop: "1.5rem" }}
        />
      </div>
    </LoadingSkeleton>
  );
}
