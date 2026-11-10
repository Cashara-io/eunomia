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

test("merge receipts distinguish confirmed redirects from a returned rejection", () => {
  const model = reviewFixture();
  const property = model.kinds[0].properties[0];
  property.status = "rejected";
  property.revision = 2;
  property.merge_into = null;
  expect(savedDecision("merge", definitions(model)[1], 1, "price")).toMatchObject({
    confirmed: false, sentence: "rejected", label: "Rejected",
  });
  property.merge_into = "price";
  expect(savedDecision("merge", definitions(model)[1], 1, "price")).toEqual({
    confirmed: true, sentence: "merged into Price", label: "Reject and redirect",
  });
});

for (const status of ["approved", "rejected", "proposed"]) {
  test(`describe receipt remains describe when status stays ${status}`, () => {
    const model = reviewFixture();
    model.kinds[0].properties[0].status = status;
    model.kinds[0].properties[0].revision = 2;
    expect(savedDecision("describe", definitions(model)[1], 1)).toEqual({
      confirmed: true, sentence: "description updated", label: "Edit description",
    });
  });
}

for (const operation of ["approve", "reject", "merge"]) {
  test(`${operation} never claims a competing response as this request's decision`, () => {
    const model = reviewFixture();
    const item = model.kinds[0].properties[0];
    item.status = operation === "approve" ? "rejected" : "approved";
    for (const revision of [2, 3]) {
      item.revision = revision;
      const result = savedDecision(operation, definitions(model)[1], 1, "price");
      expect(result.confirmed).toBe(false);
      expect(result.message).toContain(`The server returned version ${revision} as ${item.status}.`);
    }
  });
}

test("a matching status on a later revision is not confirmation of this request", () => {
  const model = reviewFixture();
  model.kinds[0].properties[0].status = "approved";
  for (const revision of [1, 3]) {
    model.kinds[0].properties[0].revision = revision;
    expect(savedDecision("approve", definitions(model)[1], 1).confirmed).toBe(false);
  }
});

test("a different returned merge destination cannot confirm the requested merge", () => {
  const model = reviewFixture();
  Object.assign(model.kinds[0].properties[0], { status: "rejected", revision: 2, merge_into: "quantity" });
  const result = savedDecision("merge", definitions(model)[1], 1, "price");
  expect(result.confirmed).toBe(false);
  expect(result.message).toContain("merged into Quantity");
});

const { comparisonRows, reviewContext } = await import("../src/components/twin/review-visual-model");

describe("review visual data", () => {
  test("context includes only direct server kinds and links, with original counts and direction", () => {
    const model = reviewFixture();
    const d = definitions(model).find(d => d.type === "property" && d.name === "unit_cost");
    const context = reviewContext(d, model);
    expect(context.kinds.map(k => k.kind)).toEqual(["stock_item", "warehouse"]);
    expect(context.links).toEqual([model.links[0]]);
    expect(context.links[0]).toMatchObject({ from_kind: "stock_item", to_kind: "warehouse", count: 120 });
    expect(context.kinds[0].entities).toBe(6000);
    model.links = [];
    expect(reviewContext(d, model).links).toEqual([]);
    expect(reviewContext(d, model).kinds.map(k => k.kind)).toEqual(["stock_item"]);
  });
  test("comparison never invents a previous approved definition", () => {
    const d = definitions(reviewFixture())[1];
    expect(comparisonRows(undefined, d).every(row => row.change === "Unavailable" && row.previous === undefined)).toBe(true);
  });
  test("known revisions distinguish added, removed, changed and unchanged fields", () => {
    const model = reviewFixture();
    const before = definitions(structuredClone(model))[1];
    model.kinds[0].properties[0].unit = null;
    model.kinds[0].properties[0].synonyms = ["cost"];
    model.kinds[0].properties[0].description = "Includes freight";
    const rows = comparisonRows(before, definitions(model)[1]);
    expect(rows.find(r => r.field === "Unit").change).toBe("Removed");
    expect(rows.find(r => r.field === "Aliases").change).toBe("Added");
    expect(rows.find(r => r.field === "Description").change).toBe("Changed");
    expect(rows.find(r => r.field === "Data type").change).toBe("Unchanged");
  });
});

const { renderToStaticMarkup } = await import("react-dom/server");
const { createElement } = await import("react");
const { default: ReviewComparison } = await import("../src/components/twin/ReviewComparison");

test("new definitions render proposal labels without a comparison or change markers", () => {
  const after = definitions(reviewFixture())[1];
  const html = renderToStaticMarkup(createElement(ReviewComparison, { after }));
  expect(html).toContain('aria-label="New definition"');
  expect(html).toContain("There is nothing to compare against yet.");
  expect(html).not.toContain('role="table"');
  expect(html).not.toContain("data-change");
});

test("an approved baseline retains current and proposed values with real differences", () => {
  const model = reviewFixture();
  model.kinds[0].properties[0].status = "approved";
  const before = definitions(structuredClone(model))[1];
  model.kinds[0].properties[0].unit = null;
  model.kinds[0].properties[0].description = "Includes freight";
  model.kinds[0].properties[0].synonyms = ["cost"];
  const html = renderToStaticMarkup(createElement(ReviewComparison, { before, after: definitions(model)[1] }));
  expect(html).toContain('aria-label="Definition comparison"');
  expect(html).toContain("Current");
  expect(html).toContain("Proposed");
  expect(html).toContain('data-change="Added"');
  expect(html).toContain('data-change="Removed"');
  expect(html).toContain('data-change="Changed"');
  expect(html).toContain("Includes freight");
  expect(html).toContain("<del>AUD</del>");
});

const { proposalEvent } = await import("../src/components/twin/review-activity");
test("inbox activity matches the server link name including both endpoints and revision", () => {
  const link = definitions(reviewFixture()).find(d => d.type === "link");
  const event = {
    action: "proposed", revision: 1, actor_name: "Claude Code", at: "2026-11-10T03:32:00Z", reason: null,
    definition_type: "link", kind: link.kind, name: link.name, target: "ontology_link:fixture",
  };
  expect(proposalEvent([event], link)).toEqual(event);
  expect(proposalEvent([{ ...event, name: "stock_item stored_at another_kind" }], link)).toBeUndefined();
  expect(proposalEvent([{ ...event, revision: 2 }], link)).toBeUndefined();
});
