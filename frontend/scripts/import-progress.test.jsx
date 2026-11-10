import { expect, test } from "bun:test";
import { importProgress } from "../src/components/twin/Activity";

test("import checkpoints show committed rows and the full file total", () => {
  expect(importProgress({ processed: 500, total: 12000, values: 1500 })).toBe("500 of 12,000 rows processed");
  expect(importProgress({ processed: 12000, total: 12000 })).toBe("12,000 of 12,000 rows processed");
  expect(importProgress({ values: 1500 })).toBeNull();
});
