"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useHttpsStatus, useRequestHttps } from "@/lib/queries/settings";
import { PanelHead } from "./shared";

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
      <div className="flex flex-col gap-5" aria-hidden>
        <PanelHead title="HTTPS" />
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
        <PanelHead title="HTTPS" />
        <div className="ledger p-5 text-[13px] max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
          HTTPS is set up by the <code className="font-mono">updater</code> service, which hasn&apos;t reported in yet. See
          Settings, Updates.
        </div>
      </div>
    );
  }

  const tone = status.state === "active" ? "var(--good)" : status.state === "error" ? "var(--critical)" : status.state === "pending" ? "var(--warning)" : "var(--ink-faint)";
  const line =
    status.state === "active"
      ? "Active"
      : status.state === "pending"
        ? `Pending: getting a certificate for ${status.domain}…`
        : status.state === "error"
          ? "Error"
          : "Off";

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="HTTPS">
        Serve Eunomia at your own domain with a free Let&apos;s Encrypt certificate, renewed automatically. Needs a domain
        whose DNS points at this machine and ports 80 and 443 reachable from the internet. Turning it on closes port 3000 to
        the network: use the domain from other machines. Only an instance admin can change it.
      </PanelHead>

      <div className="ledger p-5 flex flex-wrap items-center justify-between gap-3">
        <span className="flex items-center gap-2 text-[13px] min-w-0">
          <span className="dot" style={{ background: tone }} aria-hidden />
          <span role="status">{line}</span>
          {status.state === "active" && status.domain && (
            <a href={`https://${status.domain}`} className="font-mono text-[12px] underline" style={{ color: "var(--accent-text)" }}>
              https://{status.domain}
            </a>
          )}
        </span>
        {status.state !== "off" && (
          <button onClick={() => submit(false)} disabled={request.isPending} className="btn btn-sm shrink-0">
            Disable
          </button>
        )}
      </div>
      {status.message && (
        <p className="text-[12.5px]" style={{ color: status.state === "error" ? "var(--critical)" : "var(--ink-dim)" }}>
          {status.message}
        </p>
      )}

      <div className="flex flex-col gap-3 max-w-md">
        <label className="label flex flex-col gap-1.5">
          Domain
          <input className="field h-10 px-3 text-[14px] font-mono" value={domain} onChange={(e) => setDomain(e.target.value)} placeholder="eunomia.example.com" />
        </label>
        <label className="label flex flex-col gap-1.5">
          Email for Let&apos;s Encrypt
          <input
            type="email"
            className="field h-10 px-3 text-[14px] font-mono"
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
