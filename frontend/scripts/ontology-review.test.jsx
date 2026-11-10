import { describe, expect, test } from "bun:test";
import {
  affected,
  mergeConflicts,
  mergeTargets,
  reviewFields,
  savedDecision,
  supportsMerge,
} from "../src/components/twin/review-model";
import { definitions } from "../src/components/twin/model";
import { reviewFixture } from "../tests/fixtures/ontology-review";

describe("review evidence", () => {
  test("an unapproved destination warns that redirected writes may be refused", () => {
    const model = reviewFixture();
    model.kinds[0].properties[1].status = "proposed";
    const items = definitions(model);
    expect(mergeConflicts(items[1], items[2])).toContain(
      "The destination is not approved. Future writes or calls may be refused until an owner approves it.",
    );
  });
  test("counts distinguish historical values from objects and future calls", () => {
    const items = definitions(reviewFixture());
    expect(affected(items[0])).toBe("6,000 existing objects");
    expect(affected(items[1])).toBe("6,000 recorded values, including history");
    expect(affected(items[4])).toBe("Future calls to this metric");
  });
  test("merge candidates stay within type and kind and exclude rejected destinations", () => {
    const model = reviewFixture();
    model.kinds[0].properties[1].status = "rejected";
    const source = definitions(model)[1];
    expect(mergeTargets(source, model).map((t) => t.name)).toEqual([
      "quantity",
    ]);
  });
  test("different datatypes, units, choices and descriptions are explicit conflicts", () => {
    const model = reviewFixture();
    const source = definitions(model)[1];
    const target = definitions(model)[3];
    const conflicts = mergeConflicts(source, target);
    expect(conflicts).toContain("Data types differ: money and integer.");
    expect(conflicts).toContain("Units differ: AUD and none.");
    expect(conflicts).toContain(
      "Descriptions differ. The destination keeps its own description.",
    );
  });
  test("stale comparisons include source, samples, choices, unit and reason", () => {
    const source = definitions(reviewFixture())[1];
    expect(reviewFields(source)).toMatchObject({
      "Data type": "money",
      Unit: "AUD",
      Choices: "None",
      "Source mapping": "stock_item.csv: csv, unit_cost",
      Samples: "AUD 12.00; AUD 13.20",
    });
  });
});


test("links have no merge destinations even with matching endpoints", () => {
  const model = reviewFixture();
  model.links.push({ ...model.links[0], label: "delivered_to" });
  const source = definitions(model).find(d => d.type === "link");
  expect(mergeTargets(source, model)).toEqual([]);
});


test("merge support matches kind, property, metric and link decisions", () => {
  const items = definitions(reviewFixture());
  for (const item of items)
    expect(supportsMerge(item)).toBe(item.type !== "link");
});

test("merge receipts report the returned redirect", () => {
  const model = reviewFixture();
  const property = model.kinds[0].properties[0];
  property.status = "rejected";
  property.merge_into = null;
  expect(savedDecision("merge", definitions(model)[1])).toEqual({
    sentence: "rejected", label: "Reject without merging",
  });
  property.merge_into = "price";
  expect(savedDecision("merge", definitions(model)[1])).toEqual({
    sentence: "merged into Price", label: "Reject and redirect",
  });
});

for (const status of ["approved", "rejected", "proposed"]) {
  test(`describe receipt remains describe when status stays ${status}`, () => {
    const model = reviewFixture();
    model.kinds[0].properties[0].status = status;
    expect(savedDecision("describe", definitions(model)[1])).toEqual({
      sentence: "description updated", label: "Edit description",
    });
  });
}
