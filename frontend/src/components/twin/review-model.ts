import type { OntologyOut } from "@/lib/queries/ontology";
import { definitions, type Definition } from "./model";
import { humanLabel } from "./review-format";

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
          From: humanLabel(d.item.from_kind),
          Relationship: humanLabel(d.item.label),
          To: humanLabel(d.item.to_kind),
        }
      : {
          Aliases: d.item.synonyms.join(", ") || "None",
          "Merge destination": d.item.merge_into ? humanLabel(d.item.merge_into) : "None",
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

export function supportsMerge(d: Definition) {
  switch (d.type) {
    case "kind":
    case "property":
    case "metric":
      return true;
    case "link":
      return false;
  }
}

export function savedDecision(before: Definition, after: Definition) {
  if (
    before.item.description !== after.item.description &&
    before.item.status === after.item.status
  )
    return { sentence: "description updated", label: "Edit description" };
  if (after.item.status === "approved")
    return { sentence: "approved", label: "Approve" };
  if (after.item.status === "rejected") {
    const redirect = after.type !== "link" ? after.item.merge_into : null;
    return redirect
      ? {
          sentence: `merged into ${humanLabel(redirect)}`,
          label: "Reject and redirect",
        }
      : { sentence: "rejected", label: "Reject without merging" };
  }
  return after.item.status === "proposed"
    ? { sentence: "still suggested", label: "Suggested" }
    : { sentence: "in use", label: "In use" };
}

export function mergeTargets(d: Definition, data: OntologyOut) {
  if (!supportsMerge(d)) return [];
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
    (target.isContentEditable || Boolean(target.closest(
      'input, textarea, select, [role="textbox"], [role="combobox"], [role="searchbox"]',
    )))
  );
}

export function hasReviewOverlay(includeMerge = true) {
  return [...document.querySelectorAll<HTMLElement>(
    'dialog[open], select:open, [aria-modal="true"], :popover-open, [role="listbox"], [role="menu"], [aria-haspopup][aria-expanded="true"]' +
      (includeMerge ? ', [data-merge-open="true"]' : ''),
  )].some(element => {
    const style = getComputedStyle(element);
    return !element.hidden && style.display !== "none" && style.visibility !== "hidden";
  });
}

export function ignoreReviewKey(
  event: KeyboardEvent,
  region: HTMLElement | null,
  allowEscape = false,
) {
  return (
    event.defaultPrevented || event.shiftKey || event.ctrlKey || event.altKey ||
    event.metaKey || event.repeat || event.isComposing ||
    (isTyping(event.target) && !(allowEscape && event.key === "Escape")) ||
    !region?.contains(document.activeElement) ||
    !region.contains(event.target instanceof Node ? event.target : null)
  );
}
