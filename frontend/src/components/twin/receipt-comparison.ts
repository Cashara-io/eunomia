import { formatted } from "./value-format";
import type { PropertyReceipt } from "./receipt-model";

type Value = NonNullable<PropertyReceipt["value"]>;
type Comparable = { key: string; display: string };
type Column = Pick<PropertyReceipt, "datatype" | "unit">;

function amount(value: Value, column: Column) {
  if (typeof value === "boolean") return null;
  let text = String(value).trim();
  const sign = /^[+-]/.exec(text)?.[0] ?? "";
  if (sign) text = text.slice(1).trim();
  if (column.unit && column.unit !== "%") {
    const units = new Set([column.unit]);
    if (column.datatype === "money" && /^[A-Z]{3}$/.test(column.unit)) {
      for (const locale of ["en-AU", "en-US"]) {
        for (const currencyDisplay of ["symbol", "narrowSymbol"] as const) {
          const symbol = new Intl.NumberFormat(locale, { style: "currency", currency: column.unit, currencyDisplay })
            .formatToParts(0).find(part => part.type === "currency")?.value;
          if (symbol) units.add(symbol);
        }
      }
    }
    for (const unit of [...units].sort((a, b) => b.length - a.length)) {
      if (text.toLowerCase().startsWith(unit.toLowerCase())) {
        text = text.slice(unit.length).trim();
        break;
      }
      if (text.toLowerCase().endsWith(unit.toLowerCase())) {
        text = text.slice(0, -unit.length).trim();
        break;
      }
    }
  }
  text = sign + text;
  const percent = column.datatype === "percent" && text.endsWith("%");
  if (percent) text = text.slice(0, -1).trim();
  if (text.includes(",")) {
    if (!/^[+-]?\d{1,3}(?:,\d{3})+(?:\.\d*)?$/.test(text)) return null;
    text = text.replaceAll(",", "");
  }
  const parts = /^([+-]?)(\d+(?:\.\d*)?|\.\d+)(?:e([+-]?\d+))?$/i.exec(text);
  if (!parts) return null;
  const fraction = parts[2].split(".")[1]?.length ?? 0;
  const digits = parts[2].replace(".", "").replace(/^0+/, "").replace(/0+$/, "");
  const trailing = parts[2].replace(".", "").match(/0+$/)?.[0].length ?? 0;
  const exponent = BigInt(parts[3] ?? "0") - BigInt(fraction) + BigInt(trailing) - (percent ? BigInt(2) : BigInt(0));
  return { key: digits ? `${parts[1] === "-" ? "-" : ""}${digits}e${exponent}` : "0", text, percent };
}

function calendarDay(value: string): string | null {
  const iso = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  const dayFirst = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(value);
  let year: number, month: number, day: number;
  if (iso) [year, month, day] = iso.slice(1).map(Number);
  else if (dayFirst) {
    [day, month, year] = dayFirst.slice(1).map(Number);
    // The API supplies no import date order. Only accept an unambiguous day-first cell.
    if (day <= 12 && day !== month) return null;
  } else return null;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (month < 1 || month > 12 || day < 1 || day > days[month - 1]) return null;
  return `${String(year).padStart(4, "0")}-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
}

function instant(value: string): string | null {
  const parts = /^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(Z|[+-]\d{2}:\d{2})$/i.exec(value);
  if (!parts || !calendarDay(parts[1]) || Number(parts[2]) > 23 || Number(parts[3]) > 59 || Number(parts[4]) > 59) return null;
  const seconds = Date.parse(`${parts[1]}T${parts[2]}:${parts[3]}:${parts[4]}${parts[6]}`);
  if (!Number.isFinite(seconds)) return null;
  return `${seconds}:${(parts[5] ?? "").replace(/0+$/, "")}`;
}

function comparable(value: Value, column: Column): Comparable | null {
  if (["number", "integer", "money", "percent"].includes(column.datatype)) {
    const number = amount(value, column);
    if (!number) return null;
    // Float conversion is for display only. The comparison key retains every decimal digit.
    const displayNumber = Number(number.text) / (number.percent ? 100 : 1);
    return { key: number.key, display: Number.isFinite(displayNumber) ? formatted(displayNumber, column) : String(value) };
  }
  if (["bool", "boolean"].includes(column.datatype)) {
    const text = String(value).trim().toLowerCase();
    const boolean = ["true", "yes", "1"].includes(text) ? true : ["false", "no", "0"].includes(text) ? false : null;
    return boolean === null ? null : { key: String(boolean), display: formatted(boolean, column) };
  }
  if (typeof value !== "string") return null;
  const text = value.trim().normalize("NFC");
  if (!text) return null;
  if (column.datatype === "date" || column.datatype === "datetime") {
    const key = column.datatype === "date" ? calendarDay(text) : instant(text);
    return key === null ? null : { key, display: column.datatype === "date" ? formatted(key, column) : text };
  }
  return { key: text.toLowerCase().normalize("NFC"), display: text };
}

export function compareReceiptValues(recorded: Value, current: string, column: Column) {
  const left = comparable(recorded, column), right = comparable(current, column);
  if (!left || !right) return { kind: "uncomparable" } as const;
  if (left.key === right.key) return { kind: "match" } as const;
  // Show the original precision when ordinary formatting would hide the difference.
  const sameDisplay = left.display === right.display;
  return { kind: "different", recorded: sameDisplay ? String(recorded) : left.display, current: sameDisplay ? current.trim() : right.display } as const;
}
