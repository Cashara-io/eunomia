"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import {
  useDecideKind,
  useDecideLink,
  useDecideMetric,
  useDecideProperty,
  useTryMetric,
  type OntologyOut,
} from "@/lib/queries/ontology";
import {
  KindRow,
  LinkRow,
  MetricRow,
  PropertyRow,
  words,
} from "./DefinitionRows";
import type { Definition as DefinitionItem } from "./model";

export default function Definition({
  definition: d,
  data,
  isOwner,
  locked = false,
}: {
  definition: DefinitionItem;
  data: OntologyOut;
  isOwner: boolean;
  locked?: boolean;
}) {
  const kind = useDecideKind(data.vault);
  const link = useDecideLink(data.vault);
  const property = useDecideProperty(data.vault);
  const metric = useDecideMetric(data.vault);
  const preview = useTryMetric(data.vault);
  const [error, setError] = useState<Failure | null>(null);
  const busy =
    locked ||
    kind.isPending ||
    link.isPending ||
    property.isPending ||
    metric.isPending;
  async function run(fn: () => Promise<unknown>, fallback: string) {
    setError(null);
    try {
      await fn();
      return true;
    } catch (e) {
      setError(failure(e, fallback));
      return false;
    }
  }
  const parent = data.kinds.find((k) => k.kind === d.kind);
  return (
    <div className="flex flex-col gap-3 min-w-0 [overflow-wrap:anywhere]">
      {error && <ErrorLine error={error} />}
      <ul aria-label="Definition details">
        {d.type === "kind" && (
          <KindRow
            item={d.item}
            isOwner={isOwner}
            busy={busy}
            targets={data.kinds
              .filter(
                (k) =>
                  k.kind !== d.kind &&
                  k.status !== "rejected" &&
                  (k.built_in || k.status === "approved" || k.entities > 0),
              )
              .map((k) => ({ value: k.kind, label: words(k.kind) }))}
            onDecide={(decision, fallback) =>
              run(() => kind.mutateAsync({ kind: d.kind, decision }), fallback)
            }
          />
        )}
        {d.type === "property" && (
          <PropertyRow
            item={d.item}
            isOwner={isOwner}
            busy={busy}
            targets={(parent?.properties ?? [])
              .filter((p) => p.name !== d.name && p.status !== "rejected")
              .map((p) => ({ value: p.name, label: words(p.name) }))}
            onDecide={(decision, fallback) =>
              run(
                () =>
                  property.mutateAsync({
                    kind: d.kind,
                    name: d.name,
                    decision,
                  }),
                fallback,
              )
            }
          />
        )}
        {d.type === "metric" && (
          <MetricRow
            item={d.item}
            isOwner={isOwner}
            busy={busy}
            targets={(parent?.metrics ?? [])
              .filter((m) => m.name !== d.name && m.status !== "rejected")
              .map((m) => ({ value: m.name, label: words(m.name) }))}
            onDecide={(decision, fallback) =>
              run(
                () =>
                  metric.mutateAsync({ kind: d.kind, name: d.name, decision }),
                fallback,
              )
            }
            onTry={() => preview.mutateAsync({ kind: d.kind, name: d.name })}
          />
        )}
        {d.type === "link" && (
          <LinkRow
            item={d.item}
            isOwner={isOwner}
            busy={busy}
            onDecide={(decision, fallback) =>
              run(
                () =>
                  link.mutateAsync({
                    from_kind: d.item.from_kind,
                    label: d.item.label,
                    to_kind: d.item.to_kind,
                    decision,
                  }),
                fallback,
              )
            }
          />
        )}
      </ul>
      <Evidence definition={d} />
    </div>
  );
}

export function Evidence({ definition: d }: { definition: DefinitionItem }) {
  const item = d.item;
  const samples =
    d.type === "metric"
      ? []
      : d.type === "link"
        ? d.item.samples.map((s) => `${s.from} → ${s.to}`)
        : d.item.samples;
  const sources =
    d.type === "property" || d.type === "link" ? (d.item.source_map ?? []) : [];
  const synonyms = "synonyms" in item ? item.synonyms : [];
  const target = "merge_into" in item ? item.merge_into : null;
  const consequences =
    d.type === "kind"
      ? "Approval lets AI apps use this kind in strict mode. Rejection refuses new writes unless you explicitly choose a merge target."
      : d.type === "property"
        ? "Only approved properties accept typed values. Rejection refuses new values unless you explicitly choose a merge target."
        : d.type === "metric"
          ? "Only approved metrics run by name for AI apps. A rejected metric cannot run unless it has an explicit replacement."
          : "Approval lets AI apps use this link type in strict mode. Rejection refuses new links of this type in strict mode.";
  return (
    <dl className="grid gap-4 text-[13px]">
      <div>
        <dt className="font-medium">Why it was suggested</dt>
        <dd className="mt-1" style={{ color: "var(--ink-dim)" }}>
          {item.reason || "No reason recorded."}
        </dd>
      </div>
      <div>
        <dt className="font-medium">Source and examples</dt>
        <dd
          className="mt-1 flex flex-col gap-1"
          style={{ color: "var(--ink-dim)" }}
        >
          <span>
            {item.origin === "agent"
              ? "Proposed by an AI app."
              : item.origin === "suggested"
                ? "Suggested by the server model."
                : item.origin === "human"
                  ? "Added by a person."
                  : "Origin not recorded."}
          </span>
          {sources.length > 0 ? (
            <ul className="list-disc pl-5 max-h-36 overflow-auto">
              {sources.map((s, i) => (
                <li key={i}>
                  {s.source}: {s.record_type}, {s.path}
                </li>
              ))}
            </ul>
          ) : (
            <span>No source mapping recorded.</span>
          )}
          {samples.length > 0 ? (
            <ul className="list-disc pl-5 max-h-36 overflow-auto">
              {samples.map((s, i) => (
                <li key={i}>{s}</li>
              ))}
            </ul>
          ) : (
            <span>No sample values recorded.</span>
          )}
        </dd>
      </div>
      {synonyms.length > 0 && (
        <div>
          <dt className="font-medium">Aliases</dt>
          <dd>{synonyms.join(", ")}</dd>
        </div>
      )}
      <div>
        <dt className="font-medium">Consequences</dt>
        <dd className="mt-1" style={{ color: "var(--ink-dim)" }}>
          {consequences} Approval checks the definition, not every stored value.
        </dd>
      </div>
      <div>
        <dt className="font-medium">Merge target</dt>
        <dd className="mt-1" style={{ color: "var(--ink-dim)" }}>
          {d.type === "link"
            ? "Link types cannot be merged."
            : target
              ? `Suggested replacement: ${words(target)}. Choose Reject and merge to apply it. Batch rejection does not merge.`
              : "None suggested. Rejection alone does not merge data."}
        </dd>
      </div>
      {(d.type === "property" || d.type === "metric" || d.type === "link") && (
        <div>
          <dt className="font-medium">Depends on</dt>
          <dd>
            {d.type === "link"
              ? `${words(d.item.from_kind)} and ${words(d.item.to_kind)}`
              : words(d.kind)}
            . Approve the required kind before approving this definition.
          </dd>
        </div>
      )}
    </dl>
  );
}
