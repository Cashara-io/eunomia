import type { OntologyOut } from "@/lib/queries/ontology";
import { ReadinessPanel, STATUS } from "../DefinitionRows";
import { visibleKind } from "../model";
import styles from "../overview.module.css";

/** Definitions by status and the readiness rule, kept from the old Overview so the stage drops nothing. */
export default function StructureCounts({ vaultId, data }: { vaultId: string; data: OntologyOut }) {
  const groups = [
    { label: "Kinds", items: data.kinds.filter(visibleKind) },
    { label: "Properties", items: data.kinds.flatMap((k) => k.properties) },
    { label: "Link types", items: data.links },
    { label: "Metrics", items: data.kinds.flatMap((k) => k.metrics) },
  ];
  return (
    <details className={styles.detail}>
      <summary>
        Structure counts and readiness rule{" "}
        <span className={styles.caption}>Definition approval is not value verification</span>
      </summary>
      <div className={styles.detailBody}>
        <div role="region" aria-label="Structure by status" tabIndex={0} className={styles.tableRegion}>
          <table className="data-table w-full">
            <caption className="sr-only">Definitions by status</caption>
            <thead>
              <tr>
                <th>Definition</th>
                {Object.entries(STATUS).map(([key, s]) => (
                  <th key={key}>{key === "observed" ? "Used, not reviewed" : s.label}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {groups.map((g) => (
                <tr key={g.label}>
                  <th scope="row">{g.label}</th>
                  {Object.keys(STATUS).map((s) => (
                    <td key={s} className="font-mono">{g.items.filter((i) => i.status === s).length}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <h2>Ready for numbers</h2>
        <ReadinessPanel vaultId={vaultId} />
      </div>
    </details>
  );
}
