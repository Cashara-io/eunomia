"use client";

import { useState } from "react";
import { DownloadSimpleIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import SyncMark from "@/components/bits/SyncMark";
import { useMe } from "@/lib/queries/auth";
import { downloadDiagnostics } from "@/lib/api";
import { ICON, PanelHead } from "./shared";

type DownloadState = "idle" | "collecting" | "saved" | "capped";

export function DiagnosticsSection() {
  const me = useMe();
  const [state, setState] = useState<DownloadState>("idle");
  const [error, setError] = useState<Failure | null>(null);
  const busy = state === "collecting";

  async function download() {
    setState("collecting");
    setError(null);
    try {
      setState((await downloadDiagnostics()) ? "capped" : "saved");
    } catch (e) {
      setState("idle");
      setError(failure(e, "Could not download diagnostics. Check your connection, then try again."));
    }
  }

  return (
    <section className="flex flex-col gap-5" aria-label="Diagnostics">
      <PanelHead title="Support">Download a zip of filtered log metadata and system checks to share with support.</PanelHead>
      <div className="rounded-xl border border-[var(--border)] bg-[var(--surface)] p-5 flex flex-col gap-4">
        <h3 className="section-title">Diagnostics</h3>
        <p className="text-[13px] text-[var(--ink-dim)]">Includes recent activity and sync checks, with personal content and secrets left out.</p>
        <details className="text-[13px] text-[var(--ink-dim)]">
          <summary className="cursor-pointer text-[var(--ink)]">What&apos;s included</summary>
          <ul className="list-disc pl-5 mt-3 space-y-2">
            <li>App and database versions, feature flags, settings with sensitive values hidden, and system health.</li>
            <li>Connected source status, sync counts, partial runs, retry delays, timings and error categories, when available.</li>
            <li>Log timestamps and levels, fixed templates for recognised events, and bounded counts for those events. Other messages are redacted.</li>
          </ul>
          <p className="mt-3">No memory, document or chat text, emails, passwords or tokens. Up to 25 MiB. Review the zip before sending it. Nothing is sent automatically.</p>
        </details>
        {me.data?.is_admin ? (
          <button type="button" className="btn btn-primary self-start" onClick={download} disabled={busy}>
            {busy ? <SyncMark status="running" /> : <DownloadSimpleIcon {...ICON} />}
            {busy ? "Collecting diagnostics…" : "Download diagnostics"}
          </button>
        ) : (
          <p className="text-[13px] text-[var(--ink-dim)]">Only an instance admin can download diagnostics. Ask your instance admin for help.</p>
        )}
        <p role="status" className="text-[13px] text-[var(--ink-dim)]">
          {busy && "Preparing the zip and waiting for container logs. Container collection waits up to 60 seconds, in addition to system checks."}
          {state === "saved" && "Download ready. Review the zip before sharing it. The zip notes missing container files and counts backend events dropped under load."}
          {state === "capped" && "Download ready. The zip reached its size limit. See LIMITS.txt for files left out, and review it before sharing."}
        </p>
        {error && <ErrorLine error={error} />}
      </div>
    </section>
  );
}
