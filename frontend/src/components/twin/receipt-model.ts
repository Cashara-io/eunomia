import { compareReceiptValues } from "./receipt-comparison";
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
    return `${source.document.filename}${source.document.row ? `, row ${source.document.row.line} (matched by name)` : ", no unique current row"}`;
  return source?.record?.title || source?.record?.source || "Source unavailable";
}
export function safeReturn(value: string | null, space: string) {
  if (!value || !value.startsWith("/ontology/") || value.startsWith("//")) return null;
  const url = new URL(value, "https://local.invalid");
  return url.searchParams.get("space") === space ? url.pathname + url.search : null;
}

export const caseOnlyNote = "The current cell differs only by capital letters.";
export const rowUnavailable = "No unique current row. Several rows may share this name, or the row may have been removed or the document replaced.";
export const sourceColumn = (column: string) => column.toLowerCase().replaceAll(" ", "_");

export function sourceMismatch(property: PropertyReceipt, source: SourceReceipt | undefined): string | null {
  if (!sourceAvailable(source) || property.state === "unavailable" || property.source_state === "unavailable" || property.value == null) return null;
  const cells = Object.entries(source?.document?.row?.cells ?? {}).filter(([column]) => sourceColumn(column) === property.name);
  if (cells.length !== 1) return null;
  const comparison = compareReceiptValues(property.value, cells[0][1], property);
  if (comparison.kind === "match") return null;
  if (comparison.kind === "case-only") return caseOnlyNote;
  if (comparison.kind === "uncomparable") return "Can't compare the current cell with the recorded value. The cell is empty, unparseable or its type is uncertain.";
  return `The file now says ${comparison.current}. The recorded value is ${comparison.recorded}. ${property.state === "stale" ? "The source changed after this value was recorded." : "The file may have changed since this value was recorded."}`;
}
