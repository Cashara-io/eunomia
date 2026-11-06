"use client";

import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useSettings, useUpdateSettings } from "@/lib/queries/settings";

/* The bottom of Connected sources: how scheduled syncs behave. Saves at once, one switch. */
export default function SyncSettings() {
  const settings = useSettings().data;
  const updateSettings = useUpdateSettings();
  const [error, setError] = useState<Failure | null>(null);

  async function setCheckConnectivity(on: boolean) {
    setError(null);
    try {
      await updateSettings.mutateAsync({ sync_check_connectivity: on });
    } catch (e) {
      setError(failure(e, "Could not save the setting. Try again."));
    }
  }

  if (!settings) return null;
  return (
    <section aria-labelledby="sync-settings" className="flex flex-col gap-3 border-t pt-6">
      <h2 id="sync-settings" className="section-title">
        Sync settings
      </h2>
      <label htmlFor="check-connectivity" className="flex items-start gap-3 cursor-pointer max-w-[68ch]">
        <input
          id="check-connectivity"
          type="checkbox"
          className="mt-1"
          style={{ accentColor: "var(--accent)" }}
          checked={settings.sync_check_connectivity}
          disabled={updateSettings.isPending}
          onChange={(e) => setCheckConnectivity(e.target.checked)}
        />
        <span className="flex flex-col gap-0.5">
          <span className="text-[14px] font-medium">Skip syncs while offline</span>
          <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Before each scheduled sync, check for an internet connection. With none, the sync is skipped quietly and runs again once
            you are back online.
          </span>
        </span>
      </label>
      {error && <ErrorLine error={error} />}
    </section>
  );
}
