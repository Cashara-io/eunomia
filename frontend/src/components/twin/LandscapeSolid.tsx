import styles from "./overview.module.css";

export default function LandscapeSolid({
  x,
  y,
  height,
  shadow,
}: {
  x: number;
  y: number;
  height: number;
  shadow?: string;
}) {
  return (
    <>
      <ellipse
        cx={x + 8}
        cy={y + 16}
        rx="91"
        ry="34"
        fill={shadow ? `url(#${shadow})` : "var(--border)"}
        opacity={shadow ? 1 : 0.25}
      />
      <path
        className={styles.baseRing}
        d={`M${x - 74} ${y - 10}l101 -17 50 30 -102 18Z`}
      />
      <g className={styles.solid}>
        <path
          className={styles.faceLeft}
          d={`M${x - 62} ${y - height - 9}l38 24v${height}l-38 -24Z`}
        />
        <path
          className={styles.faceRight}
          d={`M${x - 24} ${y - height + 15}l88 -14v${height}l-88 14Z`}
        />
        <path
          className={styles.faceTop}
          d={`M${x - 62} ${y - height - 9}l88 -14 38 24 -88 15Z`}
        />
      </g>
    </>
  );
}
