import { FileTextIcon, ArrowsLeftRightIcon, FingerprintIcon, CheckCircleIcon } from "@phosphor-icons/react/ssr";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { Definition as DefinitionItem } from "./model";
import { humanLabel } from "./review-format";
import ReviewContext from "./ReviewContext";
import ReviewComparison from "./ReviewComparison";
import { affected } from "./review-model";
import styles from "./review.module.css";

export default function ReviewEvidence({
  definition: d,
  data,
  sectionPrefix,
}: {
  definition: DefinitionItem;
  data: OntologyOut;
  sectionPrefix: string;
}) {
  return (
    <>
      <section id={`${sectionPrefix}-changes`} className={styles.section} aria-label="What changes">
        <h4>
          <ArrowsLeftRightIcon size={18} aria-hidden />What changes
        </h4>
        <p className={styles.dim}>{affected(d)}.</p>
        <ReviewComparison before={d.item.status === "proposed" ? undefined : d} after={d} />
      </section>
      <Evidence definition={d} data={data} sectionPrefix={sectionPrefix} />
    </>
  );
}

export function Evidence({
  definition: d,
  data,
  sectionPrefix,
}: {
  definition: DefinitionItem;
  data?: OntologyOut;
  sectionPrefix?: string;
}) {
  const samples =
    d.type === "metric"
      ? []
      : d.type === "link"
        ? d.item.samples.map((s) => `${s.from} → ${s.to}`)
        : d.item.samples;
  const sources =
    d.type === "property" || d.type === "link" ? (d.item.source_map ?? []) : [];
  const dependencies =
    data?.kinds.filter(
      (k) =>
        k.kind === d.kind || (d.type === "link" && k.kind === d.item.to_kind),
    ) ?? [];
  return (
    <>
      <section id={sectionPrefix ? `${sectionPrefix}-source` : undefined} className={styles.section} aria-label="Source evidence">
        <h4>
          <FingerprintIcon size={18} aria-hidden />Read the source
        </h4>
        <p className={styles.dim}>
          {d.item.reason || "No suggestion reason recorded."}
        </p>
        {sources.map((s, i) => (
          <div className={styles.source} key={i}>
            <FileTextIcon size={24} weight="duotone" />
            <div>
              <p className={styles.sourceLabel}>Source file / record</p><p className="font-medium">{s.source}</p>
              <p className={styles.dim}>
                <span className={styles.sourceLabel}>Record type · Column / path</span><span>{s.record_type} · {s.path}</span>
              </p>
            </div>
          </div>
        ))}
        {sources.length === 0 && (
          <p className={`${styles.dim} mt-3`}>No source mapping recorded.</p>
        )}
        {samples.length > 0 ? (
          <table className={styles.samples}>
            <caption>
              {samples.length} recorded samples. Samples do not verify all
              values. Original row references unavailable.
            </caption>
            <thead>
              <tr>
                <th scope="col">Sample</th>
                <th scope="col">Recorded example</th>
              </tr>
            </thead>
            <tbody>
              {samples.map((s, i) => (
                <tr key={i}>
                  <td>{i + 1}</td>
                  <td>{s}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className={`${styles.dim} mt-3`}>No sample values recorded.</p>
        )}
      </section>
      {data && <ReviewContext definition={d} data={data} sectionId={sectionPrefix ? `${sectionPrefix}-context` : undefined} />}
      <section id={sectionPrefix ? `${sectionPrefix}-consequence` : undefined} className={styles.section} aria-label="Consequence of approving">
        <h4>
          <CheckCircleIcon size={18} aria-hidden />If you approve
        </h4>
        <div className={styles.consequence}>
          <p>
            {d.type === "property"
              ? `AI apps can save typed ${humanLabel(d.name)} values for ${humanLabel(d.kind)}.`
              : d.type === "metric"
                ? "AI apps can run this metric by name."
                : d.type === "link"
                  ? "AI apps can use this link type in strict mode."
                  : "AI apps can use this kind in strict mode."}{" "}
            Approval checks the definition, not every stored value.
          </p>
          <p className={`${styles.dim} mt-2`}>
            Rejecting without a merge refuses new{" "}
            {d.type === "metric" ? "calls" : "writes"} under this definition
            {d.type === "kind" || d.type === "link" ? " in strict mode" : ""}.
          </p>
        </div>
        {d.type !== "kind" && (
          <div className={`${styles.fields} mt-4`}>
            {dependencies.map((k) => (
              <span className={styles.dependency} data-state={k.built_in || k.status === "approved" ? "available" : "required"} key={k.kind}>
                {humanLabel(k.kind)}:{" "}
                {k.built_in || k.status === "approved"
                  ? "available"
                  : "approval required"}
              </span>
            ))}
            <p className={styles.dim}>
              Required kinds must be approved first. The server checks
              dependencies.
            </p>
          </div>
        )}
      </section>
    </>
  );
}
