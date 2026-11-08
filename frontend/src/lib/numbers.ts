export const isPercent = (unit: string | null | undefined) => !!unit && ["%", "percent", "percentage", "pct"].includes(unit.trim().toLowerCase());

const isMoney = (unit: string | null | undefined) => !!unit && /^[A-Z]{3}$/.test(unit);

function roundedUnits(value: number, places: number): number {
  const scaled = Math.abs(value) * 10 ** places;
  // Absorb binary error at a decimal half without moving large whole amounts.
  return Math.sign(value) * Math.round(scaled + Math.min(Number.EPSILON * scaled, 0.25));
}

function fixed(value: number, places: number): string {
  const rounded = roundedUnits(value, places) / 10 ** places;
  const finite = Number.isFinite(rounded) ? rounded : value;
  return (finite === 0 ? 0 : finite).toFixed(places);
}

function significantPlaces(value: number): number {
  return value === 0 ? 0 : Math.max(0, 1 - Math.floor(Math.log10(Math.abs(value))));
}

export function formatMoney(value: number): string {
  return fixed(value, value !== 0 && Math.abs(value) < 0.01 ? significantPlaces(value) : 2);
}

export function formatNumber(value: number, unit: string | null | undefined): string {
  if (isMoney(unit)) return formatMoney(value);
  return String(Number(fixed(value, isPercent(unit) ? 1 : 2)));
}

export const withUnit = (value: number, unit: string | null | undefined) =>
  isPercent(unit) ? `${formatNumber(value, unit)}%` : `${formatNumber(value, unit)}${unit ? ` ${unit}` : ""}`;

export function formatScenario(before: number | null, after: number | null, pct: number | null, unit: string | null) {
  if (before == null || after == null) {
    return {
      before: before == null ? "none" : formatNumber(before, unit),
      after: after == null ? "none" : formatNumber(after, unit),
      movement: after == null ? "no value in the what-if" : "no value before",
    };
  }
  const delta = after - before;
  let places = Math.max(isPercent(unit) ? 1 : 2, significantPlaces(delta));
  // Keep the raw delta, increasing precision when independently rounded endpoints do not add up.
  while (places < 12 && roundedUnits(after, places) - roundedUnits(before, places) !== roundedUnits(delta, places)) places++;
  const points = fixed(Math.abs(delta), places);
  const share = pct == null ? "" : ` (${formatNumber(Math.abs(pct), "%")}%)`;
  const amount = isPercent(unit) ? `${points} ${Number(points) === 1 ? "point" : "points"}` : `${points}${unit ? ` ${unit}` : ""}${share}`;
  return {
    before: fixed(before, places),
    after: fixed(after, places),
    movement: delta === 0 ? "no change" : `${delta > 0 ? "up" : "down"} ${amount}`,
  };
}
