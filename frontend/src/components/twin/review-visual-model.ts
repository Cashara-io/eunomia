import type { OntologyOut } from "@/lib/queries/ontology";
import type { Definition } from "./model";
import { reviewFields } from "./review-model";

export function reviewContext(d: Definition, data: OntologyOut) {
  const nearby = data.links.filter(l => l.from_kind === d.kind || l.to_kind === d.kind);
  const links = d.type === "link"
    ? [d.item, ...nearby.filter(l => l !== d.item)].slice(0, 2)
    : nearby.slice(0, 2);
  const names = [...new Set([d.kind, ...links.flatMap(l => [l.from_kind, l.to_kind])])];
  return { links, kinds: data.kinds.filter(k => names.includes(k.kind)), total: nearby.length };
}

export function comparisonRows(before: Definition | undefined, after: Definition) {
  const current = before && reviewFields(before);
  return Object.entries(reviewFields(after)).map(([field, proposed]) => {
    const previous = current?.[field];
    const empty = (value: string) => !value || value === "None" || value === "None recorded";
    const change = previous === undefined ? "Unavailable"
      : previous === proposed ? "Unchanged"
      : empty(previous) ? "Added" : empty(proposed) ? "Removed" : "Changed";
    return { field, previous, proposed, change };
  });
}
