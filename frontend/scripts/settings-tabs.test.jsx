import { expect, mock, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";

// The Settings page for each kind of person: which tabs they see. The sections themselves are stubbed (this
// is about who sees what, not what each tab shows).
// These module mocks replace the modules for the whole test process, so `package.json` runs this file in a
// `bun test` of its own: in one process with the other files, whichever loaded after it got a `next/navigation`
// with only the two hooks below.
let me = {};
mock.module("next/navigation", () => ({ useRouter: () => ({ replace() {} }), useSearchParams: () => new URLSearchParams("") }));
mock.module("../src/lib/queries/auth", () => ({ useMe: () => ({ data: me }) }));
mock.module("../src/lib/queries/settings", () => ({ useSettings: () => ({ data: { fixture: true } }) }));
const stub = () => null;
for (const [file, names] of Object.entries({
  AccessibilitySection: ["AccessibilitySection"], TokensSection: ["TokensSection"], ConnectedAppsSection: ["ConnectedAppsSection"],
  SessionsSection: ["SessionsSection"], MfaSection: ["MfaSection"], DiagnosticsSection: ["DiagnosticsSection"], UpdateSection: ["UpdateSection"],
  BackupsSection: ["BackupsSection"], HttpsSection: ["HttpsSection"], GeneralSection: ["ModelSection"], DataSection: ["DataSection"],
  UsageSection: ["UsageSection"], AuditSection: ["AuditSection"], PeopleSection: ["PeopleSection"], AccessReviewsSection: ["AccessReviewsSection"],
  OrgSection: ["OrgSection"],
})) {
  mock.module(`../src/app/(app)/settings/${file}`, () => Object.fromEntries(names.map((n) => [n, stub])));
}
const { default: SettingsPage } = await import("../src/app/(app)/settings/page");
const { SETTINGS_GROUPS, visibleGroups } = await import("../src/lib/settingsTabs");

/** The tab ids the page shows (the desktop side nav: one `role="tab"` button each). */
function tabsFor(viewer) {
  me = viewer;
  const html = renderToStaticMarkup(<SettingsPage />);
  return [...html.matchAll(/role="tab" id="tab-([a-z-]+)"/g)].map((m) => m[1]);
}

const ACCOUNT = ["apps", "tokens", "sessions", "security", "accessibility", "support", "data"];
const ORG_TABS = ["people", "org", "reviews", "audit"];
const SERVER = ["https", "updates", "backups", "model"];

test("a plain member sees only the account tabs", () => {
  expect(tabsFor({ is_admin: false, can_manage_org: false, org_role: "member" })).toEqual(ACCOUNT);
});

test("an organisation owner or admin who is not an instance admin sees People, Organisation, Access reviews and the Audit log, and no server tab", () => {
  for (const org_role of ["owner", "admin"]) {
    const tabs = tabsFor({ is_admin: false, can_manage_org: true, org_role });
    expect(tabs).toEqual([...ACCOUNT, ...ORG_TABS]);
    // operator-only controls stay hidden
    for (const hidden of [...SERVER, "usage"]) expect(tabs).not.toContain(hidden);
  }
});

test("an instance admin sees everything", () => {
  const tabs = tabsFor({ is_admin: true, can_manage_org: true, org_role: "owner" });
  expect(tabs).toEqual([...ACCOUNT, "people", "org", "reviews", "audit", "usage", ...SERVER]);
});

test("a member who is somehow flagged neither admin nor manager keeps the account tabs", () => {
  expect(tabsFor({ is_admin: false, can_manage_org: false, org_role: "admin" })).toEqual(ACCOUNT);
});

test("visibleGroups keeps the Server group away from an organisation manager and the Workspace group away from a member", () => {
  const labels = (who) => visibleGroups(SETTINGS_GROUPS, who).map((g) => g.label);
  expect(labels({ isAdmin: false, canManageOrg: false })).toEqual(["Account"]);
  expect(labels({ isAdmin: false, canManageOrg: true })).toEqual(["Account", "Workspace"]);
  expect(labels({ isAdmin: true, canManageOrg: true })).toEqual(["Account", "Workspace", "Server"]);
});
