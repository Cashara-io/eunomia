import type { ObjectColumn } from "@/lib/queries/objects";
import { formatPropertyNumber } from "@/lib/numbers";
export function formatted(
  value: string | number | boolean | null | undefined,
  column: Pick<ObjectColumn, "datatype" | "unit">,
): string {
  if (value === null || value === undefined) return "Missing";
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number")
    return formatPropertyNumber(value, column.datatype, column.unit);
  if (column.datatype === "date")
    return Number.isNaN(Date.parse(value))
      ? "Unavailable"
      : new Date(value).toLocaleDateString("en-AU", {
          dateStyle: "medium",
          timeZone: "UTC",
        });
  if (column.datatype === "datetime") return !value || value === "NONE" || Number.isNaN(Date.parse(value)) ? "Date not recorded" : new Date(value).toLocaleString("en-AU", { dateStyle: "medium", timeStyle: "short" });
  return value;
}
