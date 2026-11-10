import type { RefObject } from "react";
import { ArrowsMergeIcon } from "@phosphor-icons/react/ssr";
import { words } from "./DefinitionRows";
import type { Definition } from "./model";
import { affected, mergeConflicts } from "./review-model";
import styles from "./review.module.css";

export default function MergePreview({
  definition: d,
  targets,
  targetId,
  selectRef,
  disabled,
  onTargetChange,
}: {
  definition: Definition;
  targets: Definition[];
  targetId: string;
  selectRef: RefObject<HTMLSelectElement | null>;
  disabled: boolean;
  onTargetChange: (id: string) => void;
}) {
  const target = targets.find((t) => t.id === targetId);
  return (
    <section className={styles.section} aria-label="Merge preview">
      <h4>
        <ArrowsMergeIcon size={18} weight="duotone" />
        Merge instead
      </h4>
      <p className={`${styles.dim} mb-3`}>
        Choose a destination to inspect. Nothing is saved until you confirm.
      </p>
      <label className="flex flex-col gap-2">
        Merge {words(d.name)} into
        <select
          ref={selectRef}
          className="field p-3 w-full"
          value={targetId}
          disabled={disabled}
          onChange={(e) => onTargetChange(e.target.value)}
        >
          <option value="">Choose a destination</option>
          {targets.map((t) => (
            <option key={t.id} value={t.id}>
              {words(t.name)}
            </option>
          ))}
        </select>
      </label>
      {target && (
        <div className={`${styles.reveal} mt-5`}>
          <h4>
            {words(d.name)} → {words(target.name)}
          </h4>
          <dl className={styles.diff}>
            <div>
              <dt>Source definition</dt>
              <dd>{d.item.description || "No description recorded"}</dd>
              <dd>{affected(d)}</dd>
            </div>
            <div>
              <dt>Destination definition</dt>
              <dd>{target.item.description || "No description recorded"}</dd>
              <dd>{affected(target)}</dd>
            </div>
          </dl>
          <p className="mt-4">
            {d.type === "metric"
              ? "Future calls use the destination calculation."
              : "Future writes use the destination. Existing objects and recorded values stay where they are."}{" "}
            The source definition is rejected.
          </p>
          <h5 className="font-medium mt-4 mb-2">Conflicts to consider</h5>
          <ul className={`${styles.dim} list-disc pl-5`}>
            {mergeConflicts(d, target).map((c) => (
              <li key={c}>{c}</li>
            ))}
            <li>
              Record-level conflicts are unavailable. Check value compatibility
              before continuing.
            </li>
            <li>
              The destination keeps its own definition. No undo operation is
              available here.
            </li>
          </ul>
        </div>
      )}
    </section>
  );
}
