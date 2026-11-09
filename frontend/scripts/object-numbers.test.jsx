import { expect, test } from "bun:test";
import { formatted } from "../src/components/twin/ObjectProfile";

test("object columns keep percent and money precision, including zero", () => {
  const percent = { datatype: "percent", unit: null };
  expect([0.585, 0.4461, 0].map(v => formatted(v, percent))).toEqual(["58.50%", "44.61%", "0.00%"]);
  const money = { datatype: "money", unit: "AUD" };
  expect([12.5, 0, 0.0049, -0.0049, 1e-8].map(v => formatted(v, money)))
    .toEqual(["AUD 12.50", "AUD 0.00", "AUD 0.0049", "AUD -0.0049", "AUD 0.000000010"]);
  expect(formatted(12.345, money)).toBe("AUD 12.35");
});

test("server datatype controls ratios, counts, units and missing values", () => {
  expect(formatted(0.585, { datatype: "number", unit: "ratio" })).toBe("0.59 ratio");
  expect(formatted(1234, { datatype: "integer", unit: "units" })).toBe("1234 units");
  expect(formatted(false, { datatype: "bool", unit: null })).toBe("No");
  expect(formatted(null, { datatype: "money", unit: "AUD" })).toBe("Missing");
  expect(formatted(0.0049, { datatype: "money", unit: null })).toBe("Currency not recorded 0.0049");
});
