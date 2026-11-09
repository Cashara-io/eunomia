export const isPercent = (unit: string | null | undefined) => !!unit && ["%", "percent", "percentage", "pct"].includes(unit.trim().toLowerCase());

const isMoney = (unit: string | null | undefined) => !!unit && /^[A-Z]{3}$/.test(unit);

class Decimal {
  private constructor(readonly negative: boolean, readonly digits: string, readonly scale: number) {}

  static from(value: number): Decimal {
    // Scaling a float can move a sub-half-cent amount across the rounding boundary.
    const [mantissa, exponent = "0"] = value.toString().replace(/^-/, "").split(/e/i);
    const fraction = mantissa.split(".")[1]?.length ?? 0;
    const scale = fraction - Number(exponent);
    return Decimal.normalise(value < 0, mantissa.replace(".", "") + "0".repeat(Math.max(0, -scale)), Math.max(0, scale));
  }

  private static normalise(negative: boolean, digits: string, scale: number): Decimal {
    digits = digits.replace(/^0+/, "");
    while (scale > 0 && digits.endsWith("0")) {
      digits = digits.slice(0, -1);
      scale--;
    }
    return digits ? new Decimal(negative, digits, scale) : new Decimal(false, "0", 0);
  }

  get isZero(): boolean { return this.digits === "0"; }
  get significantPlaces(): number { return this.isZero ? 0 : Math.max(0, this.scale + 2 - this.digits.length); }
  get moneyPlaces(): number { return this.scale > this.digits.length + 1 ? this.significantPlaces : 2; }

  private render(digits: string, scale: number): string {
    digits = digits.padStart(scale + 1, "0");
    const sign = this.negative && /[1-9]/.test(digits) ? "-" : "";
    return sign + (scale === 0 ? digits : `${digits.slice(0, -scale)}.${digits.slice(-scale)}`);
  }

  percent(): Decimal { return Decimal.normalise(this.negative, this.digits + "0".repeat(Math.max(0, 2 - this.scale)), Math.max(0, this.scale - 2)); }

  exact(): string { return this.render(this.digits, this.scale); }

  fixed(places: number): string {
    if (places >= this.scale) return this.render(this.digits + "0".repeat(places - this.scale), places);
    const keep = this.digits.length - (this.scale - places);
    const digits = keep < 0 ? "0" : this.digits.slice(0, keep) || "0";
    const rounded = keep >= 0 && this.digits[keep] >= "5" ? (BigInt(digits) + BigInt(1)).toString() : digits;
    return this.render(rounded, places);
  }

  scaled(places: number): bigint | null {
    const n = BigInt(this.fixed(places).replace(".", ""));
    const limit = BigInt(2) ** BigInt(127);
    return n >= -limit && n < limit ? n : null;
  }

  static difference(before: Decimal, after: Decimal): Decimal {
    const scale = Math.max(before.scale, after.scale);
    const integer = (d: Decimal) => BigInt(`${d.negative ? "-" : ""}${d.digits}${"0".repeat(scale - d.scale)}`);
    const delta = integer(after) - integer(before);
    return Decimal.normalise(delta < 0, delta.toString().replace(/^-/, ""), scale);
  }
}

function fixed(value: number, places: number): string {
  return Number.isFinite(value) ? Decimal.from(value).fixed(places) : "none";
}

export function formatMoney(value: number): string {
  if (!Number.isFinite(value)) return "none";
  const decimal = Decimal.from(value);
  return decimal.fixed(decimal.moneyPlaces);
}

export function formatNumber(value: number | null, unit: string | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "none";
  if (isMoney(unit)) return formatMoney(value);
  return fixed(value, isPercent(unit) ? 1 : 2).replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
}

export const withUnit = (value: number | null, unit: string | null | undefined) =>
  isPercent(unit) ? `${formatNumber(value, unit)}%` : `${formatNumber(value, unit)}${unit ? ` ${unit}` : ""}`;

/** Fixed column precision. Ontology percent storage is a ratio; metric % is already points. */
export function formatPropertyNumber(value: number, datatype: string, unit: string | null): string {
  if (!Number.isFinite(value)) return "Unavailable";
  const decimal = Decimal.from(value);
  if (datatype === "percent") {
    // Shift decimal digits, rather than multiplying a binary float before rounding.
    const points = decimal.percent();
    return `${points.fixed(2)}%`;
  }
  if (datatype === "money") {
    const places = value !== 0 && Math.abs(value) < 0.01 ? Math.max(2, decimal.significantPlaces) : 2;
    return `${unit || "Currency not recorded"} ${decimal.fixed(places)}`;
  }
  const amount = datatype === "integer" ? fixed(value, 0) : decimal.fixed(2);
  return `${amount}${unit ? ` ${unit}` : ""}`;
}

export function formatScenario(before: number | null, after: number | null, pct: number | null, unit: string | null) {
  before = before != null && Number.isFinite(before) ? before : null;
  after = after != null && Number.isFinite(after) ? after : null;
  if (before == null || after == null) {
    return {
      before: formatNumber(before, unit),
      after: formatNumber(after, unit),
      movement: before == null && after == null ? "no value" : after == null ? "no value in the what-if" : "no value before",
      exactDelta: null,
      addsUp: false,
    };
  }
  const a = Decimal.from(before);
  const b = Decimal.from(after);
  const delta = Decimal.difference(a, b);
  const deltaUnit = isPercent(unit) ? "points" : unit;
  const exactDelta = `${delta.exact()}${deltaUnit ? ` ${deltaUnit}` : ""}`;
  const addsUp = (places: number) => {
    const start = a.scaled(places), end = b.scaled(places), change = delta.scaled(places);
    const limit = BigInt(2) ** BigInt(127);
    if (start == null || end == null || change == null) return false;
    const difference = end - start;
    return difference >= -limit && difference < limit && difference === change;
  };
  let places = Math.min(6, Math.max(isPercent(unit) ? 1 : 2, delta.significantPlaces));
  while (places < 6 && !addsUp(places) && a.scaled(places) != null && b.scaled(places) != null) places++;
  const guaranteed = delta.significantPlaces <= 6 && addsUp(places);
  const points = delta.fixed(places).replace(/^-/, "");
  const share = pct == null || !Number.isFinite(pct) ? "" : ` (${formatNumber(Math.abs(pct), "%")}%)`;
  const noun = /^1(?:\.0+)?$/.test(points) ? "point" : "points";
  const amount = isPercent(unit) ? `${points} ${noun}` : `${points}${unit ? ` ${unit}` : ""}${share}`;
  return {
    before: a.fixed(places),
    after: b.fixed(places),
    movement: delta.isZero ? "no change" : guaranteed ? `${delta.negative ? "down" : "up"} ${amount}` : `exact change: ${exactDelta}`,
    exactDelta,
    addsUp: guaranteed,
  };
}
