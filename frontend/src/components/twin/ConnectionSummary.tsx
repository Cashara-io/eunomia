import { useRef } from "react";
import { humanLabel, plural } from "./overview-format";
import { useLandscapeTint } from "./landscape-shared";
import type { Scene } from "./connections-model";
import styles from "./connections.module.css";
export default function ConnectionSummary({
  scene,
  focus,
  highlight,
}: {
  scene: Scene;
  focus: string;
  highlight: (kind: string) => void;
}) {
  const { tint } = useLandscapeTint();
  const focusedKind = useRef("");
  const counts = new Map<string, number>();
  for (const node of scene.nodes)
    if (node.id !== focus)
      counts.set(node.kind, (counts.get(node.kind) ?? 0) + 1);
  const max = Math.max(1, ...counts.values());
  return (
    <aside className={styles.inspector} aria-label="Neighbourhood summary">
      <span className="label">Around this object</span>
      <h3>
        {scene.nodes.length - 1} {plural("neighbour", scene.nodes.length - 1)}
      </h3>
      <p>
        {counts.size} {plural("kind", counts.size)} · {scene.edges.length}{" "}
        recorded {plural("link", scene.edges.length)}
      </p>
      <div className={styles.kindBars}>
        {[...counts]
          .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
          .map(([kind, count]) => (
            <button
              key={kind}
              style={tint(kind)}
              onMouseEnter={() => highlight(focusedKind.current || kind)}
              onMouseLeave={() => highlight(focusedKind.current)}
              onFocus={() => {
                focusedKind.current = kind;
                highlight(kind);
              }}
              onBlur={() => {
                focusedKind.current = "";
                highlight("");
              }}
              aria-label={`Highlight ${plural(humanLabel(kind))}: ${count}`}
            >
              <span>
                <span>{plural(humanLabel(kind))}</span>
                <span>{count}</span>
              </span>
              <span className={styles.barTrack}>
                <span
                  className={styles.barFill}
                  style={{ width: `${(count / max) * 100}%` }}
                />
              </span>
            </button>
          ))}
      </div>
      <p>Select an object to read its values and their sources.</p>
      <p className={styles.help}>
        Counts and heights use this loaded neighbourhood. Other links may exist
        beyond it.
      </p>
    </aside>
  );
}
