"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, Download, RefreshCw } from "lucide-react";
import { updateStatusQuery, useCheckForUpdate, useRequestUpdate } from "@/lib/queries/settings";
import type { UpdateStatus } from "@/lib/types";
import SyncMark from "@/components/bits/SyncMark";
import { ICON, CopyField, relativeTime, PanelHead, StatusLine, type Tone } from "./shared";

// The version this frontend was built as; the updater's report, when there is one, wins.
const BUILT_VERSION = process.env.NEXT_PUBLIC_APP_VERSION ?? null;

/* "Running 2.0.9 · latest 2.1.0": always the running version, and the latest when known. */
function Versions({ running, latest }: { running: string | null; latest?: string | null }) {
  return (
    <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
      Running <span className="font-mono" style={{ color: "var(--ink)" }}>{running ?? "unknown"}</span>
      {latest && latest !== running && (
        <>
          {" · latest "}
          <span className="font-mono" style={{ color: "var(--ink)" }}>{latest}</span>
        </>
      )}
    </p>
  );
}

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
        <PanelHead title="Updates" status={<span className="skeleton block h-4 w-24" />} />
        <Versions running={BUILT_VERSION} />
      </div>
    );
  }

  if (!status.configured) {
    return (
      <div className="flex flex-col gap-5">
        <PanelHead title="Updates" status={<StatusLine tone="warning">Waiting for the updater</StatusLine>} />
        <Versions running={BUILT_VERSION} />
        <div className="flex flex-col gap-3 max-w-[62ch]">
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            The updater ships with Eunomia and reports in within a minute of starting. If this stays, your install predates it: run
            the installer once more from your install folder. It updates in place and keeps your data and settings.
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
            : "Up to date";
  const tone: Tone = busy || done ? "accent" : status.update_available ? "warning" : "good";
  // The real step, from what the updater reports: it writes "Waiting for Eunomia to start" once the new images are up.
  const restartingNow = phase === "restarting" || (phase === "applying" && /waiting for eunomia to start/i.test(status.progress ?? ""));
  const step = done ? 3 : restartingNow ? 2 : phase === "applying" ? 1 : 0;

  return (
    <div className="flex flex-col gap-5">
      <PanelHead
        title="Updates"
        status={<StatusLine tone={tone}>{line}</StatusLine>}
        action={
          <button onClick={checkNow} disabled={checking || busy} className="btn btn-sm shrink-0" aria-label="Check for updates">
            {checking || checkResult ? <SyncMark status={checking ? "running" : checkResult!} /> : <RefreshCw {...ICON} />}
            {checking ? "Checking…" : "Check now"}
          </button>
        }
      >
        Checked {relativeTime(status.checked_at)}. Updating takes about a minute and your data stays put.
      </PanelHead>

      <div className="flex flex-col gap-3">
        <Versions running={status.current_version || BUILT_VERSION} latest={status.latest_version} />
        {status.update_available && !busy && !done && (
          <div className="flex flex-wrap items-center gap-2">
            <button onClick={requestUpdate} className="btn btn-primary">
              <Download {...ICON} />
              Update now
            </button>
            <a href={notes} target="_blank" rel="noopener noreferrer" className="btn btn-ghost btn-sm" style={{ color: "var(--accent-text)" }}>
              What&apos;s new
            </a>
          </div>
        )}
        {(busy || done) && <UpdateSteps current={step} detail={phase === "applying" ? (status.progress ?? null) : null} />}
      </div>

      {stale && !busy && (
        <p className="text-[13px]" style={{ color: "var(--warning)" }}>
          The updater hasn&apos;t reported for a while. Check that the <code className="font-mono">updater</code> container is
          running (<code className="font-mono">docker compose ps</code>).
        </p>
      )}
      {status.error && !busy && <ErrorLine>Last update attempt failed: {status.error}. Check the updater logs, then retry.</ErrorLine>}
      {error && <ErrorLine error={error} />}
    </div>
  );
}

const STEPS = ["Requested", "Downloading the new release", "Restarting Eunomia", "Reloading this page"] as const;

/* Where the update is, step by step. Only the current step is "in progress"; earlier ones are done. */
function UpdateSteps({ current, detail }: { current: number; detail: string | null }) {
  return (
    <ol className="flex flex-col gap-2" aria-label="Update progress">
      {STEPS.map((label, i) => {
        const state = i < current ? "done" : i === current ? "now" : "later";
        return (
          <li key={label} className="flex items-start gap-2.5 text-[13px]" aria-current={state === "now" ? "step" : undefined}>
            <span className="mt-[3px] shrink-0 w-3.5 h-3.5 flex items-center justify-center" aria-hidden>
              {state === "done" ? (
                <Check size={14} strokeWidth={2} style={{ color: "var(--good)" }} />
              ) : (
                <span className="dot" style={{ background: state === "now" ? "var(--accent)" : "var(--border-strong)" }} />
              )}
            </span>
            <span className="flex flex-col gap-0.5 min-w-0">
              <span style={{ color: state === "later" ? "var(--ink-faint)" : "var(--ink)", fontWeight: state === "now" ? 600 : 400 }}>
                {label}
                <span className="sr-only">{state === "done" ? " (done)" : state === "now" ? " (in progress)" : ""}</span>
              </span>
              {state === "now" && detail && (
                <span className="text-[12px] break-words" style={{ color: "var(--ink-dim)" }}>
                  {detail}
                </span>
              )}
            </span>
          </li>
        );
      })}
    </ol>
  );
}
