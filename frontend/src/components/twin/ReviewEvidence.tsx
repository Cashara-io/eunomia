import { FileTextIcon } from "@phosphor-icons/react/ssr";
import { STATUS, words } from "./DefinitionRows";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { Definition as DefinitionItem } from "./model";
import { affected } from "./review-model";
import styles from "./review.module.css";

export default function ReviewEvidence({
  definition: d,
  data,
}: {
  definition: DefinitionItem;
  data: OntologyOut;
}) {
  return (
    <>
      <section className={styles.section} aria-label="What changes">
        <h4>
          <span className={styles.step}>01</span>What changes
        </h4>
        <dl className={styles.diff}>
          <div>
            <dt>Current definition</dt>
            <dd>
              {d.item.status === "proposed"
                ? "Previous approved definition unavailable."
                : `${STATUS[d.item.status].label}: ${d.item.description || "No description recorded."}`}
            </dd>
            <dd className={`${styles.dim} mt-2`}>
              {affected(d)}.{" "}
              {d.item.status === "proposed" &&
                "The earlier definition cannot be compared here."}
            </dd>
          </div>
          <div>
            <dt>
              {d.item.status === "proposed"
                ? "Proposed definition"
                : "Definition under review"}
            </dt>
            <dd>{d.item.description || "No description recorded."}</dd>
            <dd className={styles.fields}>
              {d.type === "property" && (
                <>
                  <span className={styles.tag}>{words(d.item.datatype)}</span>
                  {d.item.unit && (
                    <span className={styles.tag}>{d.item.unit}</span>
                  )}
                  {d.item.enum_values?.length ? (
                    <span className={styles.tag}>
                      {d.item.enum_values.join(", ")}
                    </span>
                  ) : null}
                </>
              )}
              {d.type === "metric" && (
                <span>
                  {d.item.expr_words}
                  {d.item.unit ? ` · ${d.item.unit}` : ""}
                </span>
              )}
              {d.type === "link" && (
                <span>
                  {words(d.item.from_kind)} → {words(d.item.to_kind)}
                </span>
              )}
              {d.type !== "link" && d.item.synonyms.length > 0 && (
                <span>Also called {d.item.synonyms.join(", ")}</span>
              )}
            </dd>
          </div>
        </dl>
      </section>
      <Evidence definition={d} data={data} />
    </>
  );
}

export function Evidence({
  definition: d,
  data,
}: {
  definition: DefinitionItem;
  data?: OntologyOut;
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
      <section className={styles.section} aria-label="Source evidence">
        <h4>
          <span className={styles.step}>02</span>Read the source
        </h4>
        <p className={styles.dim}>
          {d.item.reason || "No suggestion reason recorded."}
        </p>
        {sources.map((s, i) => (
          <div className={styles.source} key={i}>
            <FileTextIcon size={24} weight="duotone" />
            <div>
              <p className="font-medium">{s.source}</p>
              <p className={styles.dim}>
                {s.record_type} · {s.path}
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
      <section className={styles.section} aria-label="Consequence of approving">
        <h4>
          <span className={styles.step}>03</span>If you approve
        </h4>
        <div className={styles.consequence}>
          <p>
            {d.type === "property"
              ? `AI apps can save typed ${words(d.name)} values for ${words(d.kind)}.`
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
              <span className={styles.tag} key={k.kind}>
                {words(k.kind)}:{" "}
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
