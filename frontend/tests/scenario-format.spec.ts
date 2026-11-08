import { test, expect } from "@playwright/test";
import { formatNumber, formatScenario } from "../src/lib/numbers";

test("scenario precision preserves raw moves and endpoint arithmetic", () => {
  for (const [before, after, unit, expectedBefore, expectedAfter, movement] of [
    [35.756120, 35.435683, "%", "35.76", "35.44", "down 0.32 points"],
    [38.062307, 33.895036, "%", "38.1", "33.9", "down 4.2 points"],
    [10, 10.004, "AUD", "10.0000", "10.0040", "up 0.0040 AUD"],
    [10, 9.996, null, "10.0000", "9.9960", "down 0.0040"],
    [1.004, 1.006, "AUD", "1.0040", "1.0060", "up 0.0020 AUD"],
    [35.754, 35.435, "%", "35.754", "35.435", "down 0.319 points"],
  ] as const) {
    const shown = formatScenario(before, after, null, unit);
    expect(shown.before).toBe(expectedBefore);
    expect(shown.after).toBe(expectedAfter);
    expect(shown.movement).toBe(movement);
  }
});

test("money rounds half away from zero", () => {
  for (const [value, expected] of [[68606.625, "68606.63"], [-68606.625, "-68606.63"], [1.005, "1.01"], [-1.005, "-1.01"], [1e15, "1000000000000000.00"]] as const) {
    expect(formatNumber(value, "AUD")).toBe(expected);
  }
});

test("a nonzero sub-cent value is not shown as zero", () => {
  expect(formatNumber(0.004, "AUD")).not.toBe("0.00");
});
