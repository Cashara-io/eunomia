import { formatted } from "./value-format";
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

export const rowUnavailable = "No unique current row. Several rows may share this name, or the row may have been removed or the document replaced. The API cannot distinguish these cases.";
export const sourceColumn = (column: string) => column.toLowerCase().replaceAll(" ", "_");

function currentValue(cell: string, property: PropertyReceipt) {
  const text = cell.trim();
  if (!text) return null;
  if (["number", "integer", "money", "percent"].includes(property.datatype)) {
    let amount = text;
    if (property.unit && property.unit !== "%") {
      if (amount.startsWith(property.unit + " ")) amount = amount.slice(property.unit.length).trim();
      if (amount.endsWith(" " + property.unit)) amount = amount.slice(0, -property.unit.length).trim();
    }
    const percent = property.datatype === "percent" && amount.endsWith("%");
    if (percent) amount = amount.slice(0, -1).trim();
    if (/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(amount) && Number.isFinite(Number(amount)))
      return Number(amount) / (percent ? 100 : 1);
  }
  if (["bool", "boolean"].includes(property.datatype)) {
    if (["true", "yes", "1"].includes(text.toLowerCase())) return true;
    if (["false", "no", "0"].includes(text.toLowerCase())) return false;
  }
  return cell;
}
export function sourceMismatch(property: PropertyReceipt, source: SourceReceipt | undefined): string | null {
  if (!sourceAvailable(source) || property.state === "unavailable" || property.source_state === "unavailable" || property.value == null) return null;
  const cells = Object.entries(source?.document?.row?.cells ?? {}).filter(([column]) => sourceColumn(column) === property.name);
  if (cells.length !== 1) return null;
  const current = formatted(currentValue(cells[0][1], property), property);
  const recorded = formatted(property.value, property);
  if (current === recorded) return null;
  return `The file now says ${current}. The recorded value is ${recorded}. ${property.state === "stale" ? "The source changed after this value was recorded." : "The file may have changed since this value was recorded."}`;
}
