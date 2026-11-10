import { expect, test } from "bun:test";
import { UploadBudget, uploadPolicy } from "../src/lib/upload-budget";

test("2 MiB at 128 KiB/s and 25 MiB beyond 120 seconds stay within budget", () => {
  for (const size of [2 * 1024 * 1024, 25 * 1024 * 1024]) {
    const budget = new UploadBudget(size, uploadPolicy, 0);
    for (let sent = 0, now = 0; sent < size; sent += 128 * 1024, now += 1000) {
      budget.record(128 * 1024, now);
      expect(budget.expired(now)).toBe(false);
    }
  }
});

test("a burst does not buy a stalled or trickling sender an extended lease", () => {
  for (const rate of [0, 8 * 1024]) {
    const budget = new UploadBudget(25 * 1024 * 1024, uploadPolicy, 0);
    budget.record(2 * 1024 * 1024, 0);
    for (let now = 1000; now < 25_000; now += 1000) {
      budget.record(rate, now);
      expect(budget.expired(now)).toBe(false);
    }
    expect(budget.expired(25_000)).toBe(true);
  }
});

test("ceiling scales with size and stops at fifteen minutes", () => {
  expect(new UploadBudget(2 * 1024 * 1024, uploadPolicy, 0).ceilingMs).toBe(47_000);
  expect(new UploadBudget(100 * 1024 * 1024, uploadPolicy, 0).ceilingMs).toBe(900_000);
  const budget = new UploadBudget(2 * 1024 * 1024, uploadPolicy, 0);
  budget.record(2 * 1024 * 1024, 47_000);
  expect(budget.expired(47_000)).toBe(true);
});
