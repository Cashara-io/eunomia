import { z } from "zod";
import { formatNumber } from "@/lib/numbers";
export const rowsSchema = z.array(z.record(z.string(), z.unknown())).max(500);
export type Row = z.infer<typeof rowsSchema>[number];
export function rowsOf(data: unknown): Row[] {
  const parsed = rowsSchema.safeParse(data);
  if (parsed.success) return parsed.data;
  const wrapped = z.object({ rows: rowsSchema }).safeParse(data);
  return wrapped.success ? wrapped.data.rows : [];
}
export function number(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}
export function label(value: unknown): string {
  if (
    value &&
    typeof value === "object" &&
    "name" in value &&
    typeof value.name === "string"
  )
    return value.name;
  return value == null
    ? "none"
    : typeof value === "string" ||
        typeof value === "number" ||
        typeof value === "boolean"
      ? String(value)
      : "";
}
export function formatted(value: unknown, format?: string): string {
  if (value == null) return "none";
  if (format === "date" && typeof value === "string") {
    const date = new Date(value);
    return Number.isNaN(date.valueOf())
      ? value
      : /^\d{4}-\d{2}$/.test(value)
        ? date.toLocaleDateString(undefined, {
            month: "short",
            year: "numeric",
            timeZone: "UTC",
          })
        : date.toLocaleDateString();
  }
  const n = number(value);
  return n == null
    ? label(value)
    : format === "money"
      ? `A$${formatNumber(n, "AUD")}`
      : format === "percent"
        ? `${formatNumber(n, "%")}%`
        : formatNumber(n, null);
}
