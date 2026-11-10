import type { ObjectProfile } from "@/lib/queries/objects";
export type PropertyReceipt = ObjectProfile["properties"][number];
export type SourceReceipt = ObjectProfile["sources"][number];

export function receiptState(property: PropertyReceipt) {
  if (property.state === "unavailable") return "Source unavailable";
  if (property.state === "recorded" && property.value === 0) return "Zero";
  return {
    recorded: "Recorded", missing: "Missing", stale: "Stale", conflicting: "Conflicting",
  }[property.state];
}
export function sourceFor(data: ObjectProfile, property: PropertyReceipt) {
  // Source identity, never a filename, joins a value to its exact record.
  return data.sources.find(source => source.id === property.source);
}
export function sourceAvailable(source: SourceReceipt | undefined) {
  return source?.state === "available";
}
export function sourceLabel(source: SourceReceipt | undefined) {
  if (!sourceAvailable(source)) return "Source unavailable";
  if (source?.document)
    return `${source.document.filename}${source.document.row ? `, row ${source.document.row.line}` : ", exact row unavailable"}`;
  return source?.record?.title || source?.record?.source || "Source unavailable";
}
export function safeReturn(value: string | null, space: string) {
  if (!value || !value.startsWith("/ontology/") || value.startsWith("//")) return null;
  const url = new URL(value, "https://local.invalid");
  return url.searchParams.get("space") === space ? url.pathname + url.search : null;
}
