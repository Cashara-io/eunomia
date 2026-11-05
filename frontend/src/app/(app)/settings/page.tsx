"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { Suspense, useRef } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useSettings } from "@/lib/queries/settings";
import { SlidersHorizontal } from "lucide-react";
import PageHeader from "@/components/ui/PageHeader";

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

  const current = TABS.find((t) => t.id === tab)!;

  return (
    <div className="max-w-5xl flex flex-col gap-7">
      <PageHeader icon={SlidersHorizontal} title="Settings" intro={`Settings / ${current.label}. Change how this Eunomia runs, who can reach it and what is stored.`} />

      {!settings ? (
        loadError ? (
          <ErrorLine error={loadError} />
        ) : (
          <div className="flex flex-col gap-6" aria-hidden>
            <div className="flex flex-wrap gap-2">
              {TABS.map((t) => (
                <span key={t.id} className="skeleton h-9 w-24" />
              ))}
            </div>
            <div className="flex flex-col gap-3">
              <span className="skeleton h-4 w-40" />
              <span className="skeleton h-40 w-full" />
            </div>
          </div>
        )
      ) : (
        <div className="flex flex-col gap-7">
          <div role="tablist" aria-label="Settings sections" className="flex flex-wrap gap-2">
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
                className="pill min-h-10! px-4! text-[14px]! font-semibold whitespace-nowrap"
              >
                {t.label}
              </button>
            ))}
          </div>

          <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="min-w-0 max-w-4xl pt-6" style={{ borderTop: "1px solid var(--border)" }}>
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
