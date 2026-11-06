"use client";

import { DownloadSimpleIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import { downloadExport } from "@/lib/api";
import { ICON, PanelHead } from "./shared";

export function DataSection() {
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<Failure | null>(null);

  async function doExport() {
    setExporting(true);
    setError(null);
    try {
      await downloadExport();
    } catch (e) {
      setError(failure(e, "Could not prepare the export.", " Try again in a moment."));
    } finally {
      setExporting(false);
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="Your data">
        Download everything Eunomia holds about you (people &amp; topics, memories, how they connect, and your chat history) as one file you can keep or move elsewhere.
      </PanelHead>
      <div className="ledger px-5 py-4 flex flex-wrap items-center justify-between gap-3">
        <div>
          <div className="text-[15px] font-semibold">Export</div>
          <div className="label mt-0.5 font-mono">eunomia-export.json</div>
        </div>
        <button onClick={doExport} disabled={exporting} className="btn">
          <DownloadSimpleIcon {...ICON} />
          {exporting ? "Preparing…" : "Download my data"}
        </button>
      </div>
      {error && <ErrorLine error={error} />}
    </div>
  );
}
