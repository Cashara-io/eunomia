"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import { AccessibilitySection } from "./AccessibilitySection";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { Suspense, useEffect, useRef } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useSettings } from "@/lib/queries/settings";
import { useMe } from "@/lib/queries/auth";
import PageHeader from "@/components/ui/PageHeader";

import { TokensSection } from "./TokensSection";
import { ConnectedAppsSection } from "./ConnectedAppsSection";
import { SessionsSection } from "./SessionsSection";
import { MfaSection } from "./MfaSection";
import { DiagnosticsSection } from "./DiagnosticsSection";
import { UpdateSection } from "./UpdateSection";
import { BackupsSection } from "./BackupsSection";
import { HttpsSection } from "./HttpsSection";
import { ModelSection } from "./GeneralSection";
import { DataSection } from "./DataSection";
import { UsageSection } from "./UsageSection";
import { AuditSection } from "./AuditSection";
import { PeopleSection } from "./PeopleSection";
import { AccessReviewsSection } from "./AccessReviewsSection";
import { SETTINGS_GROUPS } from "@/lib/settingsTabs";

const GROUPS = SETTINGS_GROUPS;

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
  // an owner or admin of the organisation who is not an instance admin sees People (invitations) only
  const canManageOrg = me.data?.can_manage_org ?? false;
  type Group = { label: string; admin: boolean; tabs: readonly { id: TabId; label: string }[] };
  const groups: Group[] = (GROUPS as readonly Group[]).flatMap((g) =>
    isAdmin || !g.admin ? [g] : canManageOrg && g.label === "Workspace" ? [{ ...g, tabs: g.tabs.filter((t) => t.id === "people") }] : []
  );
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
          <LoadingSkeleton label="Loading settings" layout="settings" rows={5} />
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
                        className="text-left px-3 py-1.5 rounded-[10px] text-[14px] transition-colors hover:bg-[var(--surface-hover)]"
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
            {tab === "accessibility" && <AccessibilitySection />}
            {tab === "tokens" && <TokensSection />}
            {tab === "sessions" && <SessionsSection />}
            {tab === "security" && <MfaSection />}
            {tab === "support" && <DiagnosticsSection />}
            {tab === "data" && <DataSection />}
            {tab === "people" && <PeopleSection full={isAdmin} />}
            {tab === "reviews" && <AccessReviewsSection />}
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
