import { test, expect } from "@playwright/test";
import { execFileSync } from "node:child_process";
import type { ScenarioView } from "../src/components/chat/views/types";

test("scenario card keeps the exact delta in its title and the rounded line visible", () => {
  const metric = { metric: "score", kind: "sample", expr_words: "Average score", unit: "%", n_entities: 1, status: "approved" } as const;
  const view = {
    view: "scenario", headline: "Score changes", text: "",
    result: {
      vault: "vault:test", as_of: "2026-11-09T00:00:00Z", changes: [],
      baseline: [{ ...metric, value: 35.7 }],
      scenario: [{ ...metric, value: 35.351, delta: -0.349, delta_pct: null }],
      changed: [], n_changed: 0, removed: [], reached: [], reached_truncated: false, warnings: [],
    },
  } satisfies ScenarioView;
  const html = execFileSync("bun", ["-e", `
    import { createElement } from "react";
    import { renderToStaticMarkup } from "react-dom/server";
    import ScenarioView from "./src/components/chat/views/ScenarioView";
    process.stdout.write(renderToStaticMarkup(createElement(ScenarioView, { view: ${JSON.stringify(view)} })));
  `], { encoding: "utf8" });
  expect(html).toContain('title="Exact change: -0.349 points"');
  const visible = html.replace(/<[^>]*>/g, "");
  expect(visible).toContain("down 0.35 points");
  expect(visible).not.toContain("-0.349");
});
