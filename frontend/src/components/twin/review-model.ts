import type { OntologyOut } from "@/lib/queries/ontology";
import { definitions, type Definition } from "./model";

export function reviewFields(d: Definition): Record<string, string> {
  return {
    Status: d.item.status,
    Description: d.item.description || "No description recorded",
    ...(d.type === "property"
      ? {
          "Data type": d.item.datatype,
          Unit: d.item.unit || "None",
          Choices: d.item.enum_values?.join(", ") || "None",
        }
      : {}),
    ...(d.type === "metric"
      ? { Calculation: d.item.expr_words, Unit: d.item.unit || "None" }
      : {}),
    ...(d.type === "link"
      ? {
          From: d.item.from_kind,
          Relationship: d.item.label,
          To: d.item.to_kind,
        }
      : {
          Aliases: d.item.synonyms.join(", ") || "None",
          "Merge destination": d.item.merge_into || "None",
        }),
    "Suggested reason": d.item.reason || "None",
    ...(d.type === "property" || d.type === "link"
      ? {
          "Source mapping":
            d.item.source_map
              ?.map((s) => `${s.source}: ${s.record_type}, ${s.path}`)
              .join("; ") || "None",
        }
      : {}),
    Samples:
      d.type === "metric"
        ? "None recorded"
        : d.type === "link"
          ? d.item.samples.map((s) => `${s.from} to ${s.to}`).join("; ")
          : d.item.samples.join("; "),
  };
}

export function mergeTargets(d: Definition, data: OntologyOut) {
  return definitions(data).filter(
    (t) =>
      t.type === d.type &&
      t.id !== d.id &&
      t.item.status !== "rejected" &&
      (t.type === "kind"
        ? t.item.built_in || t.item.status === "approved" || t.item.entities > 0
        : t.kind === d.kind),
  );
}

export function affected(d: Definition) {
  switch (d.type) {
    case "kind":
      return `${d.item.entities.toLocaleString("en-AU")} existing objects`;
    case "property":
      return `${d.item.values.toLocaleString("en-AU")} recorded values, including history`;
    case "link":
      return `${d.item.count.toLocaleString("en-AU")} existing links`;
    case "metric":
      return "Future calls to this metric";
  }
}

export function mergeConflicts(
  source: Definition,
  target: Definition,
): string[] {
  const conflicts: string[] = [];
  if (
    target.item.status !== "approved" &&
    !(target.type === "kind" && target.item.built_in)
  ) {
    conflicts.push(
      "The destination is not approved. Future writes or calls may be refused until an owner approves it.",
    );
  }
  if (source.type === "property" && target.type === "property") {
    if (source.item.datatype !== target.item.datatype)
      conflicts.push(
        `Data types differ: ${source.item.datatype} and ${target.item.datatype}.`,
      );
    if (source.item.unit !== target.item.unit)
      conflicts.push(
        `Units differ: ${source.item.unit || "none"} and ${target.item.unit || "none"}.`,
      );
    if (
      JSON.stringify(source.item.enum_values ?? []) !==
      JSON.stringify(target.item.enum_values ?? [])
    )
      conflicts.push(
        "Allowed choices differ. Check the destination choices before continuing.",
      );
  }
  if (
    source.type === "metric" &&
    target.type === "metric" &&
    JSON.stringify(source.item.expr) !== JSON.stringify(target.item.expr)
  )
    conflicts.push(
      "Calculations differ. Future calls will use the destination calculation.",
    );
  if (source.item.description !== target.item.description)
    conflicts.push(
      "Descriptions differ. The destination keeps its own description.",
    );
  return conflicts;
}

export function isTyping(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    Boolean(
      target.closest(
        "input, textarea, select, [contenteditable=true], [role=combobox], [role=dialog], dialog",
      ),
    )
  );
}
