// The Settings tabs, in one place: the Settings page's side nav and the command palette both read
// this list, so a tab can't exist in one and be missing (or stale) in the other.
// Account is everyone's; Workspace and Server are for instance admins only.
export const SETTINGS_GROUPS = [
  {
    label: "Account",
    admin: false,
    tabs: [
      { id: "apps", label: "Connected apps", keywords: "oauth sign-in claude chatgpt" },
      { id: "tokens", label: "API tokens", keywords: "key token mcp" },
      { id: "sessions", label: "Sessions", keywords: "devices sign out" },
      { id: "accessibility", label: "Accessibility", keywords: "haptic feedback vibration motion" },
      { id: "support", label: "Support", keywords: "diagnostics logs download help zip" },
      { id: "data", label: "Your data", keywords: "export download delete" },
    ],
  },
  {
    label: "Workspace",
    admin: true,
    tabs: [
      { id: "people", label: "People", keywords: "users accounts admin suspend sign up" },
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
