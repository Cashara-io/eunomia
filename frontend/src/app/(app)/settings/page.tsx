"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { Suspense, useEffect, useRef } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useSettings } from "@/lib/queries/settings";
import { useMe } from "@/lib/queries/auth";
import PageHeader from "@/components/ui/PageHeader";

import { TokensSection } from "./TokensSection";
import { ConnectedAppsSection } from "./ConnectedAppsSection";
import { SessionsSection } from "./SessionsSection";
import { UpdateSection } from "./UpdateSection";
import { BackupsSection } from "./BackupsSection";
import { HttpsSection } from "./HttpsSection";
import { ModelSection } from "./GeneralSection";
import { DataSection } from "./DataSection";
import { UsageSection } from "./UsageSection";
import { AuditSection } from "./AuditSection";
import { PeopleSection } from "./PeopleSection";

// Account is everyone's; Workspace and Server are for instance admins only.
const GROUPS = [
  {
    label: "Account",
    admin: false,
    tabs: [
      { id: "apps", label: "Connected apps" },
      { id: "tokens", label: "API tokens" },
      { id: "sessions", label: "Sessions" },
      { id: "data", label: "Your data" },
    ],
  },
  {
    label: "Workspace",
    admin: true,
    tabs: [
      { id: "people", label: "People" },
      { id: "audit", label: "Audit log" },
      { id: "usage", label: "Usage" },
    ],
  },
  {
    label: "Server",
    admin: true,
    tabs: [
      { id: "https", label: "HTTPS" },
      { id: "updates", label: "Updates" },
      { id: "backups", label: "Backups" },
      { id: "model", label: "AI provider" },
    ],
  },
] as const;

type TabId = (typeof GROUPS)[number]["tabs"][number]["id"];
const DEFAULT_TAB: TabId = "apps";

// useSearchParams needs a Suspense boundary for the static build.
export default function SettingsPage() {
  return (
    <Suspense>
      <Settings />
    </Suspense>
  );
}

function Settings() {
  // The tab lives in the URL (?tab=updates), so links to it, like the sidebar's
  // "Update available" badge, work from any page, this one included.
  const router = useRouter();
  const requested = useSearchParams().get("tab");
  const me = useMe();
  const isAdmin = me.data?.is_admin ?? false;
  const groups = GROUPS.filter((g) => isAdmin || !g.admin);
  const tabs = groups.flatMap((g) => g.tabs as readonly { id: TabId; label: string }[]);
  // an unknown tab, or an admin tab for a member, falls back to the first one
  const tab: TabId = tabs.find((t) => t.id === requested)?.id ?? DEFAULT_TAB;
  const settingsQuery = useSettings();
  const settings = settingsQuery.data ?? null;
  const loadError = settingsQuery.isError
    ? failure(settingsQuery.error, "Could not load settings.", " Check that the Eunomia API is running, then reload this page.")
    : null;
  const tabRefs = useRef<Partial<Record<TabId, HTMLButtonElement | null>>>({});

  function choose(id: TabId) {
    router.replace(id === DEFAULT_TAB ? "/settings" : `/settings?tab=${id}`, { scroll: false });
  }

  // General is gone (its sync switch lives on Connected sources): old links to it land on the default tab
  useEffect(() => {
    if (requested === "general") router.replace("/settings", { scroll: false });
  }, [requested, router]);

  function onKey(e: React.KeyboardEvent, id: TabId) {
    const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const i = tabs.findIndex((t) => t.id === id);
    const next = tabs[(i + step + tabs.length) % tabs.length].id;
    choose(next);
    tabRefs.current[next]?.focus();
  }

  return (
    <div className="max-w-6xl flex flex-col gap-7">
      <PageHeader title="Settings" />

      {!settings || !me.data ? (
        loadError ? (
          <ErrorLine error={loadError} />
        ) : (
          <div className="flex flex-col lg:flex-row gap-8" aria-hidden>
            <div className="flex flex-col gap-2 lg:w-[200px]">
              {[0, 1, 2, 3, 4].map((i) => (
                <span key={i} className="skeleton h-7 w-full lg:w-40" />
              ))}
            </div>
            <div className="flex-1 flex flex-col gap-3">
              <span className="skeleton h-4 w-40" />
              <span className="skeleton h-40 w-full" />
            </div>
          </div>
        )
      ) : (
        <div className="flex flex-col lg:flex-row gap-6 lg:gap-10">
          {/* Phones and tablets: one native picker. */}
          <div className="lg:hidden flex flex-col gap-1.5">
            <label htmlFor="settings-section" className="label">
              Section
            </label>
            <select
              id="settings-section"
              className="field h-10 px-3 text-[14px] w-full"
              value={tab}
              onChange={(e) => choose(e.target.value as TabId)}
            >
              {groups.map((g) =>
                groups.length > 1 ? (
                  <optgroup key={g.label} label={g.label}>
                    {g.tabs.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.label}
                      </option>
                    ))}
                  </optgroup>
                ) : (
                  g.tabs.map((t) => (
                    <option key={t.id} value={t.id}>
                      {t.label}
                    </option>
                  ))
                )
              )}
            </select>
          </div>

          {/* Desktop: a grouped sub-nav on the left. */}
          <nav aria-label="Settings sections" className="hidden lg:flex flex-col gap-6 w-[200px] shrink-0">
            {groups.map((g) => (
              <div key={g.label} className="flex flex-col gap-1">
                {groups.length > 1 && (
                  <span id={`group-${g.label}`} className="label px-3 pb-1">
                    {g.label}
                  </span>
                )}
                <div role="tablist" aria-orientation="vertical" aria-label={g.label} className="flex flex-col gap-0.5">
                  {g.tabs.map((t) => {
                    const on = tab === t.id;
                    return (
                      <button
                        key={t.id}
                        ref={(el) => {
                          tabRefs.current[t.id] = el;
                        }}
                        role="tab"
                        id={`tab-${t.id}`}
                        aria-selected={on}
                        aria-controls={`panel-${t.id}`}
                        tabIndex={on ? 0 : -1}
                        onClick={() => choose(t.id)}
                        onKeyDown={(e) => onKey(e, t.id)}
                        className="text-left px-3 py-1.5 rounded-[6px] text-[14px] transition-colors hover:bg-[var(--surface-hover)]"
                        style={
                          on
                            ? { background: "var(--surface-raised)", color: "var(--ink)", fontWeight: 600 }
                            : { color: "var(--ink-dim)" }
                        }
                      >
                        {t.label}
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
          </nav>

          <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="min-w-0 flex-1 max-w-4xl">
            {tab === "apps" && <ConnectedAppsSection />}
            {tab === "tokens" && <TokensSection />}
            {tab === "sessions" && <SessionsSection />}
            {tab === "data" && <DataSection />}
            {tab === "people" && <PeopleSection />}
            {tab === "audit" && <AuditSection />}
            {tab === "usage" && <UsageSection />}
            {tab === "https" && <HttpsSection />}
            {tab === "updates" && <UpdateSection />}
            {tab === "backups" && <BackupsSection />}
            {tab === "model" && <ModelSection settings={settings} />}
          </div>
        </div>
      )}
    </div>
  );
}
