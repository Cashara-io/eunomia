import { expect, mock, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";

// The page a member of a suspended workspace sees (WorkspaceGate). The server refuses every route but a few account
// ones while a workspace is suspended (`plan::allowed_while_suspended`), so the page must offer, and render, only
// screens that call those: the password change and "Your data". The hooks are mocked here and every call a mocked
// hook stands for is recorded in `calls`, so a test can say which routes a screen would call.
// These module mocks replace the modules for the whole test process, so `package.json` runs this file in a
// `bun test` of its own.
let me = { org_role: "member", can_manage_org: false, has_password: true };
const calls = new Set();
const query = (route, data) => () => {
  calls.add(route);
  return { data, isPending: false, isError: false };
};
const mutation = (route) => () => {
  calls.add(route);
  return { mutateAsync: async () => ({}), isPending: false };
};
mock.module("next/navigation", () => ({ useRouter: () => ({ replace() {} }) }));
mock.module("../src/lib/queries/auth", () => ({
  useMe: () => {
    calls.add("GET /api/auth/me");
    return { data: me };
  },
  useLogout: mutation("POST /api/auth/logout"),
}));
mock.module("../src/lib/queries/org", () => ({ useOrgPlan: query("GET /api/org/plan", { status: "suspended", manage_url: "https://billing.example.test/manage" }) }));
mock.module("../src/lib/queries/recovery", () => ({ useChangePassword: mutation("POST /api/account/password") }));
mock.module("../src/lib/queries/subject", () => ({
  useSubjectExports: query("GET /api/export/subject", []),
  useStartExport: mutation("POST /api/export/subject"),
  useEraseMe: mutation("POST /api/account/erasure"),
  useMyDataActivity: query("GET /api/account/activity", []),
}));
mock.module("../src/lib/queries/client", () => ({ leaveTo() {} }));
// the two downloads: the personal-space JSON is `GET /api/export` (refused while suspended), an export is `GET /api/export/subject/{id}`
const realApi = { ...(await import("../src/lib/api")) };
mock.module("../src/lib/api", () => ({
  ...realApi,
  downloadExport: async () => calls.add("GET /api/export"),
  downloadSubjectExport: async () => calls.add("GET /api/export/subject/{id}"),
}));

const { Suspended, accountViewOf } = await import("../src/components/WorkspaceGate");

// what the server lets a suspended workspace's member call: the same list as `allowed_while_suspended` in plan.rs
const ALLOWED = ["GET /api/auth/me", "GET /api/org/plan", "POST /api/account/password", "POST /api/account/erasure", "GET /api/account/activity", "GET /api/export/subject", "POST /api/export/subject", "GET /api/export/subject/{id}", "POST /api/auth/logout"];

const page = (view, manageUrl = null) => renderToStaticMarkup(<Suspended manageUrl={manageUrl} view={view} />);

test("the notice links to changing your password and to your data, as real links", () => {
  const html = page(null);
  expect(html).toContain('data-testid="workspace-suspended"');
  expect(html).toMatch(/<a href="#password"[^>]*>Change your password<\/a>/);
  expect(html).toMatch(/<a href="#data"[^>]*>Export or delete your data<\/a>/);
  // and still the way out
  expect(html).toContain("Sign out");
  // neither screen is open until its link is followed
  expect(html).not.toContain('data-testid="workspace-view-password"');
  expect(html).not.toContain('data-testid="workspace-view-data"');
});

test("the owner keeps the link to the billing page; a member does not get one", () => {
  me = { org_role: "owner", can_manage_org: true, has_password: true };
  expect(page(null, "https://billing.example.test/manage")).toMatch(/<a href="https:\/\/billing\.example\.test\/manage"[^>]*data-testid="workspace-manage"/);
  expect(page(null, null)).not.toContain('data-testid="workspace-manage"');
  me = { org_role: "member", can_manage_org: false, has_password: true };
  expect(page(null, "https://billing.example.test/manage")).not.toContain('data-testid="workspace-manage"');
});

test("the address fragment picks the screen, and anything else shows none", () => {
  expect(accountViewOf("#password")).toBe("password");
  expect(accountViewOf("#data")).toBe("data");
  for (const other of ["", "#", "#settings", "#password2", "#DATA"]) expect(accountViewOf(other)).toBeNull();
});

test("the password screen renders, and calls only routes a suspended workspace allows", () => {
  calls.clear();
  const html = page("password");
  expect(html).toContain('data-testid="workspace-view-password"');
  expect(html).toContain('data-testid="change-password"');
  expect(html).toContain("Current password");
  expect(html).toContain('aria-current="page"');
  expect(calls.size).toBeGreaterThan(0);
  for (const route of calls) expect(ALLOWED).toContain(route);
});

test("the data screen renders export and delete, and calls only routes a suspended workspace allows", () => {
  calls.clear();
  const html = page("data");
  expect(html).toContain('data-testid="workspace-view-data"');
  expect(html).toContain("Export my data");
  expect(html).toContain('data-testid="delete-account"');
  expect(html).toContain("Delete my account");
  // the personal-space JSON reads workspace content, which a suspended workspace refuses: it is not offered here
  expect(html).not.toContain("download just your personal space as JSON");
  for (const route of calls) expect(ALLOWED).toContain(route);
});

test("in Settings the same screen still offers the personal-space JSON", async () => {
  const { DataSection } = await import("../src/app/(app)/settings/DataSection");
  expect(renderToStaticMarkup(<DataSection />)).toContain("download just your personal space as JSON");
  expect(renderToStaticMarkup(<DataSection paused />)).not.toContain("download just your personal space as JSON");
});
