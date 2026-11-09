"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useHttpsStatus, useRequestHttps } from "@/lib/queries/settings";
import { PanelHead, StatusLine, type Tone } from "./shared";

const INTRO = "Serve Eunomia at your own domain with a free certificate that renews itself.";

export function HttpsSection() {
  const statusQuery = useHttpsStatus();
  const status = statusQuery.data ?? null;
  const request = useRequestHttps();
  const [domain, setDomain] = useState("");
  const [email, setEmail] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  // Prefill the domain once the updater reports one.
  const reported = status?.configured ? (status.domain ?? "") : "";
  const [seen, setSeen] = useState("");
  if (reported && reported !== seen) {
    setSeen(reported);
    setDomain((d) => d || reported);
  }

  async function submit(enable: boolean) {
    setError(null);
    try {
      await request.mutateAsync(enable ? { enabled: true, domain: domain.trim(), email: email.trim() } : { enabled: false });
    } catch (e) {
      setError(failure(e, "Could not save."));
    }
  }

  if (!status) {
    return (
      <LoadingSkeleton label="Loading https" layout="settings" />
    );
  }

  if (!status.configured) {
    return (
      <div className="flex flex-col gap-5">
        <PanelHead title="HTTPS" status={<StatusLine tone="warning">Waiting for the updater</StatusLine>}>
          {INTRO}
        </PanelHead>
        <p className="text-[13px] max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
          HTTPS is set up by the updater service, which hasn&apos;t reported in yet. See Settings, Updates.
        </p>
      </div>
    );
  }

  const tone: Tone = status.state === "active" ? "good" : status.state === "error" ? "critical" : status.state === "pending" ? "warning" : "off";
  const line =
    status.state === "active"
      ? "On"
      : status.state === "pending"
        ? `Pending: getting a certificate for ${status.domain}…`
        : status.state === "error"
          ? "Error"
          : "Off";

  return (
    <div className="flex flex-col gap-5">
      <PanelHead
        title="HTTPS"
        status={
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <StatusLine tone={tone}>{line}</StatusLine>
            {status.state === "active" && status.domain && (
              <a href={`https://${status.domain}`} className="link text-[13px]">
                https://{status.domain}
              </a>
            )}
          </span>
        }
        action={
          status.state !== "off" && (
            <button onClick={() => submit(false)} disabled={request.isPending} className="btn btn-sm shrink-0">
              Disable
            </button>
          )
        }
      >
        {INTRO}
      </PanelHead>
      {status.message && (
        <p className="text-[13px]" style={{ color: status.state === "error" ? "var(--critical)" : "var(--ink-dim)" }}>
          {status.message}
        </p>
      )}

      <div className="flex flex-col gap-3 max-w-md">
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          The domain&apos;s DNS must point at this machine, with ports 80 and 443 open to the internet. Once it is on, use the domain
          from other machines: port 3000 closes to the network.
        </p>
        <label className="label flex flex-col gap-1.5">
          Domain
          <input className="field h-10 px-3 text-[14px] tabular-nums" value={domain} onChange={(e) => setDomain(e.target.value)} placeholder="eunomia.example.com" />
        </label>
        <label className="label flex flex-col gap-1.5">
          Email for Let&apos;s Encrypt
          <input
            type="email"
            className="field h-10 px-3 text-[14px] tabular-nums"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            placeholder="you@example.com"
          />
        </label>
        <button onClick={() => submit(true)} disabled={request.isPending} className="btn btn-primary self-start">
          Enable HTTPS
        </button>
        {error && <ErrorLine error={error} />}
      </div>
    </div>
  );
}
