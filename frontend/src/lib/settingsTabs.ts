// The Settings tabs, in one place: the Settings page's side nav and the command palette both read
// this list, so a tab can't exist in one and be missing (or stale) in the other.
// Account is everyone's. Server is for instance admins only. Workspace is the organisation's own administration:
// an owner or admin of the organisation sees the tabs in ORG_MANAGER_TABS; an instance admin sees all of them.
export const SETTINGS_GROUPS = [
  {
    label: "Account",
    admin: false,
    tabs: [
      { id: "apps", label: "Connected apps", keywords: "oauth sign-in claude chatgpt" },
      { id: "tokens", label: "API tokens", keywords: "key token mcp" },
      { id: "sessions", label: "Sessions", keywords: "devices sign out" },
      { id: "security", label: "Password and two-step", keywords: "password change mfa 2fa totp authenticator recovery codes security" },
      { id: "accessibility", label: "Accessibility", keywords: "haptic feedback vibration motion" },
      { id: "support", label: "Support", keywords: "diagnostics logs download help zip" },
      { id: "data", label: "Your data", keywords: "export download delete" },
    ],
  },
  {
    label: "Workspace",
    admin: true,
    tabs: [
      { id: "people", label: "People", keywords: "users accounts admin suspend sign up members role sessions revoke" },
      { id: "org", label: "Organisation", keywords: "organization name rename security two-step verification mfa require policy" },
      { id: "reviews", label: "Access reviews", keywords: "access review recertify who has access compliance" },
      { id: "audit", label: "Audit log", keywords: "history events activity integrity verify export" },
      { id: "usage", label: "Usage", keywords: "tool calls stats" },
    ],
  },
  {
    label: "Server",
    admin: true,
    tabs: [
      { id: "https", label: "HTTPS", keywords: "tls certificate domain" },
      { id: "updates", label: "Updates", keywords: "version upgrade" },
      { id: "backups", label: "Backups", keywords: "backup restore" },
      { id: "model", label: "AI provider", keywords: "openai model key chat llm" },
    ],
  },
] as const;

export type SettingsTabId = (typeof SETTINGS_GROUPS)[number]["tabs"][number]["id"];

/** The Workspace tabs an owner or admin of the organisation sees without being an instance admin: People
 * (suspend, roles, sessions, tokens and apps), Organisation (its name and its two-step verification policy),
 * Access reviews and the Audit log, all for their own organisation. Usage and the Server group stay with
 * instance admins. */
export const ORG_MANAGER_TABS: readonly string[] = ["people", "org", "reviews", "audit"];

export type SettingsGroup = { label: string; admin: boolean; tabs: readonly { id: string; label: string; keywords?: string }[] };

/** The groups a viewer sees: everyone gets the non-admin groups; an instance admin all of them; an organisation
 * owner or admin (not an instance admin) the Workspace tabs of ORG_MANAGER_TABS. */
export function visibleGroups<G extends SettingsGroup>(groups: readonly G[], who: { isAdmin: boolean; canManageOrg: boolean }): G[] {
  return groups.flatMap((g) =>
    who.isAdmin || !g.admin
      ? [g]
      : who.canManageOrg && g.label === "Workspace"
        ? [{ ...g, tabs: g.tabs.filter((t) => ORG_MANAGER_TABS.includes(t.id)) }]
        : []
  );
}
