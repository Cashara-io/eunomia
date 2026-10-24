"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { Suspense, useRef } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useSettings } from "@/lib/queries/settings";

import { TokensSection } from "./TokensSection";
import { ConnectedAppsSection } from "./ConnectedAppsSection";
import { SessionsSection } from "./SessionsSection";
import { UpdateSection } from "./UpdateSection";
import { HttpsSection } from "./HttpsSection";
import { GeneralSection } from "./GeneralSection";
import { DataSection } from "./DataSection";
import { UsageSection } from "./UsageSection";

const TABS = [
  { id: "general", label: "General" },
  { id: "updates", label: "Updates" },
  { id: "https", label: "HTTPS" },
  { id: "tokens", label: "API tokens" },
  { id: "apps", label: "Connected apps" },
  { id: "sessions", label: "Sessions" },
  { id: "usage", label: "Usage" },
  { id: "data", label: "Your data" },
] as const;

type TabId = (typeof TABS)[number]["id"];

const isTab = (t: string | null): t is TabId => TABS.some((x) => x.id === t);

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
  const tab: TabId = isTab(requested) ? requested : "general";
  const settingsQuery = useSettings();
  const settings = settingsQuery.data ?? null;
  const loadError = settingsQuery.isError ? failure(settingsQuery.error, "Could not load settings.", " Check that the Eunomia API is running, then reload this page.") : null;
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);

  function choose(id: TabId) {
    router.replace(id === "general" ? "/settings" : `/settings?tab=${id}`, { scroll: false });
  }

  function onKey(e: React.KeyboardEvent, i: number) {
    const next = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key];
    if (!next) return;
    e.preventDefault();
    const j = (i + next + TABS.length) % TABS.length;
    choose(TABS[j].id);
    tabRefs.current[j]?.focus();
  }

  return (
    <div className="max-w-5xl flex flex-col gap-7">
      <h1 className="page-title">Settings</h1>

      {!settings ? (
        loadError ? (
          <ErrorLine error={loadError} />
        ) : (
          <div className="grid gap-8 md:grid-cols-[180px_minmax(0,1fr)]" aria-hidden>
            <div className="flex md:flex-col gap-2">
              {TABS.map((t) => (
                <span key={t.id} className="skeleton h-7 w-28" />
              ))}
            </div>
            <div className="flex flex-col gap-3">
              <span className="skeleton h-4 w-40" />
              <span className="skeleton h-40 w-full" />
            </div>
          </div>
        )
      ) : (
        <div className="grid gap-6 md:gap-10 md:grid-cols-[180px_minmax(0,1fr)] items-start">
          <div
            role="tablist"
            aria-label="Settings sections"
            aria-orientation="vertical"
            className="flex md:flex-col gap-0.5 overflow-x-auto -mx-4 px-4 md:mx-0 md:px-0 md:sticky md:top-6"
          >
            {TABS.map((t, i) => (
              <button
                key={t.id}
                ref={(el) => {
                  tabRefs.current[i] = el;
                }}
                role="tab"
                id={`tab-${t.id}`}
                aria-selected={tab === t.id}
                aria-controls={`panel-${t.id}`}
                tabIndex={tab === t.id ? 0 : -1}
                onClick={() => choose(t.id)}
                onKeyDown={(e) => onKey(e, i)}
                data-active={tab === t.id ? "" : undefined}
                className="nav-link shrink-0 h-8 px-2.5 rounded-[7px] text-[13px] text-left whitespace-nowrap transition-transform active:scale-[0.97]"
              >
                {t.label}
              </button>
            ))}
          </div>

          <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="min-w-0 max-w-3xl">
            {tab === "general" && <GeneralSection settings={settings} />}
            {tab === "updates" && <UpdateSection />}
            {tab === "https" && <HttpsSection />}
            {tab === "tokens" && <TokensSection />}
            {tab === "apps" && <ConnectedAppsSection />}
            {tab === "sessions" && <SessionsSection />}
            {tab === "usage" && <UsageSection />}
            {tab === "data" && <DataSection />}
          </div>
        </div>
      )}
    </div>
  );
}
