import { expect, test } from "bun:test";
import { UploadBudget, uploadPolicy } from "../src/lib/upload-budget";

function fixture(limit, policy = uploadPolicy) {
  let now = 75_000;
  const budget = new UploadBudget(limit, policy, () => now);
  return { budget, at: (elapsed) => { now = 75_000 + elapsed; } };
}

test("2 MiB at 128 KiB/s and 25 MiB beyond 120 seconds stay within budget", () => {
  for (const size of [2 * 1024 * 1024, 25 * 1024 * 1024]) {
    const { budget, at } = fixture(size);
    for (let sent = 0, elapsed = 0; sent < size; elapsed += 25) {
      at(elapsed);
      if (elapsed % 125 === 0) {
        budget.record(16 * 1024);
        sent += 16 * 1024;
      }
      expect(budget.expired()).toBe(false);
    }
  }
});

test("production policy tolerates one two-second pause at twice the minimum rate", () => {
  // Sweep pauses across grace, rate enforcement and bucket boundaries.
  for (let pause = 0; pause <= 40_000; pause += 125) {
    const { budget, at } = fixture(25 * 1024 * 1024);
    for (let elapsed = 0; elapsed < 60_000; elapsed += 25) {
      at(elapsed);
      if (elapsed % 125 === 0 && !(elapsed >= pause && elapsed < pause + 2000)) {
        budget.record(16 * 1024);
      }
      expect(budget.expired()).toBe(false);
    }
  }
});

test("the former 200 ms test window rejects a 225 ms scheduling gap", () => {
  const { budget, at } = fixture(2 * 1024 * 1024, { ...uploadPolicy, graceMs: 200, windowMs: 200 });
  for (const elapsed of [125, 250, 375]) {
    at(elapsed);
    budget.record(16 * 1024);
  }
  at(400);
  expect(budget.expired()).toBe(false);
  at(600);
  expect(budget.expired()).toBe(true);
});

test("a stalled sender expires at grace plus the window", () => {
  const { budget, at } = fixture(25 * 1024 * 1024);
  at(uploadPolicy.graceMs + uploadPolicy.windowMs - 1);
  expect(budget.expired()).toBe(false);
  at(uploadPolicy.graceMs + uploadPolicy.windowMs);
  expect(budget.expired()).toBe(true);
});

test("a burst does not buy a stalled or trickling sender an extended lease", () => {
  for (const burst of [0, 2 * 1024 * 1024]) {
    for (const rate of [0, 8 * 1024]) {
      const { budget, at } = fixture(25 * 1024 * 1024);
      budget.record(burst);
      for (let elapsed = 1000; elapsed <= 25_000; elapsed += 1000) {
        at(elapsed);
        budget.record(rate);
        expect(budget.expired()).toBe(elapsed === 25_000);
      }
    }
  }
});

test("repeated bursts separated by stalls cannot extend the hard ceiling", () => {
  const { budget, at } = fixture(151 * 1024 * 1024);
  for (let elapsed = 0; elapsed <= budget.ceilingMs; elapsed += 1000) {
    at(elapsed);
    if (elapsed % 8000 === 0) budget.record(1024 * 1024);
    expect(budget.expired()).toBe(elapsed === budget.ceilingMs);
  }
});

test("ceiling scales with size and stops at fifteen minutes", () => {
  expect(fixture(2 * 1024 * 1024).budget.ceilingMs).toBe(47_000);
  expect(fixture(100 * 1024 * 1024).budget.ceilingMs).toBe(900_000);
  const { budget, at } = fixture(2 * 1024 * 1024);
  at(47_000);
  budget.record(2 * 1024 * 1024);
  expect(budget.expired()).toBe(true);
});
