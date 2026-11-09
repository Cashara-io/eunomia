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
// Bounded inputs keep chart products, sums and scale differences finite.
export const MAX_DRAWABLE = 1e100;
export function requiredNumber(value: unknown): number {
  const n = number(value);
  if (n == null) throw new Error("Missing numeric measurement");
  return n;
}
export function drawable(value: unknown): boolean {
  const n = number(value);
  return n != null && Math.abs(n) <= MAX_DRAWABLE && (n === 0 || Math.abs(n) >= 1e-100);
}
// Keep reciprocal and percentage arithmetic within the shared drawable range.
export function proportionalTotal(values: unknown[]): number | null {
  if (values.some(value => !drawable(value))) return null;
  const total = values.reduce<number>((sum, value) => sum + Math.max(0, requiredNumber(value)), 0);
  return total > 0 && drawable(total) && Number.isFinite(1 / total) ? total : null;
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
export function formatted(value: unknown, format?: string, unit?: string | null): string {
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
  if (n == null && (typeof value === "number" || ["number", "money", "percent"].includes(format ?? ""))) return "none";
  return n == null
    ? label(value)
    : format === "money"
      ? `${unit === "AUD" || !unit ? "A$" : unit + " "}${formatNumber(n, unit || "AUD")}`
      : format === "percent"
        ? `${formatNumber(n, "%")}%`
        : formatNumber(n, null);
}
