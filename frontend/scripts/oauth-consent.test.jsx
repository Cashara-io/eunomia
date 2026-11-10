import { expect, mock, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";

mock.module("../src/lib/queries/oauth", () => ({
  useConsent: () => ({ data: {
    client: { name: "Fixture app" }, user_email: "fixture@example.test",
    redirect_host: "app.example.test", scopes: [
      { scope: "memory:read", description: "Read memories" },
      { scope: "memory:write", description: "Save memories" },
    ],
  } }),
  useDecideConsent: () => ({}),
}));
const { default: ConsentPage } = await import("../src/app/consent/page");

test("OAuth consent discloses read and write across Personal and team spaces before Allow", () => {
  const html = renderToStaticMarkup(<ConsentPage />);
  expect(html).toContain("Read memories in every space you belong to");
  expect(html).toContain("Save memories in every space you belong to");
  expect(html).toContain("including your Personal space and team spaces");
  expect(html.indexOf("including your Personal space and team spaces")).toBeLessThan(html.indexOf(">Allow<"));
});
