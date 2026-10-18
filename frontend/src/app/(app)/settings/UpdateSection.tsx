"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Download, RefreshCw } from "lucide-react";
import { updateStatusQuery, useCheckForUpdate, useRequestUpdate } from "@/lib/queries/settings";
import type { UpdateStatus } from "@/lib/types";
import SyncMark from "@/components/bits/SyncMark";
import { ICON, CopyField, relativeTime, PanelHead } from "./shared";

const INSTALL_CMD = "curl -fsSL https://www.buildpaperless.com.au/eunomia/install.sh | bash";

// idle -> waiting (marker dropped, updater picks it up within ~20s) -> applying
// -> restarting (API briefly unreachable) -> reload once the new version answers.
type Phase = "idle" | "waiting" | "applying" | "restarting";

export function UpdateSection() {
  const qc = useQueryClient();
  const requestUpdateMutation = useRequestUpdate();
  const checkMutation = useCheckForUpdate();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [target, setTarget] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  // How the last "Check now" ended, shown on the button for 1.2s.
  const [checkResult, setCheckResult] = useState<"done" | "failed" | null>(null);
  const [wasChecking, setWasChecking] = useState(false);
  const [stale, setStale] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const requestedAt = useRef(0);

  const busy = phase !== "idle";
  if (checking !== wasChecking) {
    setWasChecking(checking);
    if (wasChecking && !checking && !checkResult) setCheckResult("done");
  }
  useEffect(() => {
    if (!checkResult) return;
    const t = setTimeout(() => setCheckResult(null), 1200);
    return () => clearTimeout(t);
  }, [checkResult]);

  const load = useCallback(
    () =>
      // Always hit the network (staleTime 0) but share the cache with the sidebar's update badge.
      qc
        .fetchQuery({ ...updateStatusQuery(), staleTime: 0 })
        .then((s) => {
          setStatus(s);
          setError(null);
          if (!s.configured) return;
          setStale(Date.now() - new Date(s.checked_at).getTime() > 30 * 60 * 1000);
          setChecking((c) => c && Date.now() - new Date(s.checked_at).getTime() > 5000);
          const fresh = new Date(s.checked_at).getTime() > requestedAt.current; // ignore an error from an older attempt
          setPhase((p) =>
            p === "idle" ? p : s.applying ? "applying" : s.error && fresh ? "idle" : p === "waiting" ? p : "restarting"
          );
        })
        .catch(() => setPhase((p) => (p === "idle" ? p : "restarting"))),
    [qc]
  );

  useEffect(() => {
    load();
    const id = setInterval(load, busy || checking ? 3000 : 15000);
    return () => clearInterval(id);
  }, [load, busy, checking]);

  // the new release is serving: reload so the browser runs the new frontend too
  const done = Boolean(target && status?.configured && status.current_version === target && !status.applying);
  useEffect(() => {
    if (!done) return;
    const t = setTimeout(() => window.location.reload(), 1500);
    return () => clearTimeout(t);
  }, [done]);

  async function requestUpdate() {
    if (!status?.configured) return;
    setError(null);
    try {
      requestedAt.current = Date.now();
      await requestUpdateMutation.mutateAsync();
      setTarget(status.latest_version);
      setPhase("waiting");
    } catch (e) {
      setError(failure(e, "Could not request an update. Check that the updater container is running, then retry."));
    }
  }

  async function checkNow() {
    setChecking(true);
    setCheckResult(null);
    await checkMutation.mutateAsync().catch(() => {
      setChecking(false);
      setCheckResult("failed");
    });
  }

  if (!status) {
    return (
      <div className="flex flex-col gap-5" aria-hidden>
        <PanelHead title="Updates" />
        <div className="ledger p-5 flex flex-col gap-3">
          <span className="skeleton h-6 w-32" />
          <span className="skeleton h-4 w-56" />
        </div>
      </div>
    );
  }

  if (!status.configured) {
    return (
      <div className="flex flex-col gap-5">
        <PanelHead title="Updates" />
        <div className="ledger p-5 flex flex-col gap-3">
          <div className="flex items-center gap-2 text-[13px] font-medium">
            <span className="dot" style={{ background: "var(--warning)" }} aria-hidden />
            Waiting for the updater
          </div>
          <p className="text-[13px] max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
            It ships with Eunomia as the <code className="font-mono">updater</code> service and reports in within a minute of
            starting. If this message stays, this install predates it: run the installer once more from the folder that
            contains your install. It updates in place and keeps your data and settings.
          </p>
          <CopyField value={INSTALL_CMD} />
        </div>
      </div>
    );
  }

  const notes = `https://github.com/Cashara-io/eunomia/releases/tag/${encodeURIComponent(status.latest_version)}`;
  const line = done
    ? `Updated to ${target}. Reloading…`
    : phase === "waiting"
      ? "Update requested. Starting in a few seconds…"
      : phase === "applying"
        ? (status.progress ?? `Installing ${target ?? status.latest_version}…`)
        : phase === "restarting"
          ? "Restarting Eunomia…"
          : status.update_available
            ? `${status.latest_version} is available`
            : "You're on the latest release";
  const tone = busy || done ? "var(--accent)" : status.update_available ? "var(--warning)" : "var(--good)";
  const progress = done ? 1 : phase === "restarting" ? 0.8 : phase === "applying" ? 0.5 : 0.15;

  return (
    <div className="flex flex-col gap-5">
      <PanelHead
        title="Updates"
        action={
          <button onClick={checkNow} disabled={checking || busy} className="btn btn-sm shrink-0" aria-label="Check for updates">
            {checking || checkResult ? <SyncMark status={checking ? "running" : checkResult!} /> : <RefreshCw {...ICON} />}
            {checking ? "Checking…" : "Check now"}
          </button>
        }
      >
        Checked {relativeTime(status.checked_at)}. Updating takes about a minute and your data stays put.
      </PanelHead>

      <div className="ledger overflow-hidden">
        <div className="p-5 flex flex-wrap items-center justify-between gap-4">
          <div className="min-w-0 flex flex-col gap-1">
            <span className="label">Running version</span>
            <span className="text-[20px] font-mono font-medium tracking-[-0.01em]">{status.current_version}</span>
            <span className="flex items-center gap-2 text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
              <span className="dot" style={{ background: tone }} aria-hidden />
              <span role="status">{line}</span>
            </span>
          </div>
          {status.update_available && !busy && !done && (
            <div className="flex items-center gap-2 shrink-0">
              <a
                href={notes}
                target="_blank"
                rel="noopener noreferrer"
                className="btn btn-ghost btn-sm"
                style={{ color: "var(--accent-text)" }}
              >
                What&apos;s new
              </a>
              <button onClick={requestUpdate} className="btn btn-primary">
                <Download {...ICON} />
                Update now
              </button>
            </div>
          )}
        </div>
        {(busy || done) && (
          <div className="h-[3px]" style={{ background: "var(--surface-raised)" }} aria-hidden>
            <div
              className="h-full origin-left"
              style={{
                background: "var(--accent)",
                transform: `scaleX(${progress})`,
                transition: "transform 700ms var(--ease-out)",
              }}
            />
          </div>
        )}
      </div>

      {stale && !busy && (
        <p className="text-[12.5px]" style={{ color: "var(--warning)" }}>
          The updater hasn&apos;t reported for a while. Check that the <code className="font-mono">updater</code> container is
          running (<code className="font-mono">docker compose ps</code>).
        </p>
      )}
      {status.error && !busy && <ErrorLine>Last update attempt failed: {status.error}. Check the updater logs, then retry.</ErrorLine>}
      {error && <ErrorLine error={error} />}
    </div>
  );
}
