"use client";

import { useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import CopyButton from "@/components/bits/CopyButton";

/** What the server returns when enrolment starts: shown once. */
export type Enrolment = { secret: string; otpauth_uri: string; qr_svg: string };

/** The six-digit (or recovery) code field. Digits only for an authenticator code; a recovery code has letters. */
export function CodeField({
  id,
  value,
  onChange,
  label = "Code from your authenticator app",
  recovery = false,
  invalid,
  autoFocus = true,
}: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  label?: string;
  recovery?: boolean;
  invalid?: boolean;
  autoFocus?: boolean;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-[14px] font-semibold">
        {label}
      </label>
      <input
        id={id}
        className="field h-11 px-3 text-reading font-mono tracking-[0.12em] w-full max-w-[260px]"
        inputMode={recovery ? "text" : "numeric"}
        autoComplete="one-time-code"
        autoCapitalize="off"
        spellCheck={false}
        autoFocus={autoFocus}
        required
        maxLength={recovery ? 32 : 12}
        placeholder={recovery ? "xxxxx-xxxxx" : "123456"}
        aria-invalid={invalid || undefined}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

/** Scan, type the first code, done. `start` is called once, on mount; `confirm` turns MFA on and returns the recovery codes. */
export function EnrolSteps({
  start,
  confirm,
  onCodes,
  submitLabel = "Turn on two-step verification",
}: {
  start: () => Promise<Enrolment>;
  confirm: (code: string) => Promise<string[]>;
  onCodes: (codes: string[]) => void;
  submitLabel?: string;
}) {
  const [enrolment, setEnrolment] = useState<Enrolment | null>(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const started = useRef(false);

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    start().then(setEnrolment, (e) => setError(failure(e, "Could not start the setup. Reload and try again.")));
  }, [start]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onCodes(await confirm(code.trim()));
    } catch (err) {
      setError(failure(err, "That code didn't work. Check the app and try again."));
      setCode("");
      setBusy(false);
    }
  }

  if (!enrolment) {
    return error ? (
      <ErrorLine error={error} />
    ) : (
      <p role="status" className="flex items-center gap-2 text-body" style={{ color: "var(--ink-dim)" }}>
        <Loader2 size={14} className="animate-spin" aria-hidden /> Preparing your QR code…
      </p>
    );
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4" data-testid="mfa-enrol">
      <ol className="flex flex-col gap-1 text-reading list-decimal pl-5" style={{ color: "var(--ink)" }}>
        <li>Open an authenticator app (1Password, Bitwarden, Google or Microsoft Authenticator and others).</li>
        <li>Scan this QR code, or type the key below into the app.</li>
        <li>Enter the six-digit code the app shows.</li>
      </ol>
      <div className="flex flex-wrap items-start gap-5">
        {/* a standalone SVG from the server (no scripts), shown as an image so it cannot run anything */}
        {/* eslint-disable-next-line @next/next/no-img-element -- a data URI, nothing to optimise */}
        <img
          src={`data:image/svg+xml;utf8,${encodeURIComponent(enrolment.qr_svg)}`}
          alt="QR code for your authenticator app"
          width={180}
          height={180}
          className="rounded-[10px] border"
          style={{ borderColor: "var(--border)", background: "#fff" }}
        />
        <div className="flex flex-col gap-1.5 min-w-0 flex-1">
          <span className="text-[14px] font-semibold">Key, if you can&apos;t scan</span>
          <div className="field flex items-center gap-2 min-h-10 pl-3 pr-1 max-w-[360px]">
            <code data-testid="mfa-secret" className="flex-1 min-w-0 break-all text-detail font-mono" style={{ color: "var(--ink)" }}>
              {enrolment.secret}
            </code>
            <CopyButton value={enrolment.secret} size="sm" className="shrink-0" />
          </div>
          <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            It&apos;s shown only now. Once two-step verification is on, nobody can see it again.
          </p>
        </div>
      </div>
      <CodeField id="mfa-enrol-code" value={code} onChange={setCode} autoFocus={false} invalid={Boolean(error)} />
      {error && <ErrorLine error={error} />}
      <button type="submit" disabled={busy || !code.trim()} aria-busy={busy} className="btn btn-primary self-start">
        {busy && <Loader2 size={14} className="animate-spin" aria-hidden />}
        {busy ? "Checking…" : submitLabel}
      </button>
    </form>
  );
}

/** The recovery codes, shown once, with one way to keep them. Continue only after saying they're saved. */
export function RecoveryCodes({ codes, onDone, doneLabel = "Done", children }: { codes: string[]; onDone: () => void; doneLabel?: string; children?: React.ReactNode }) {
  const [saved, setSaved] = useState(false);
  return (
    <div className="flex flex-col gap-4" data-testid="mfa-recovery-codes">
      <p className="text-reading" style={{ color: "var(--ink)" }}>
        Save these recovery codes somewhere safe, like a password manager. Each one signs you in once if you lose your
        authenticator app. They won&apos;t be shown again.
      </p>
      <ul data-testid="mfa-codes-list" className="grid grid-cols-2 gap-x-6 gap-y-1.5 p-4 rounded-[10px] font-mono text-detail max-w-[360px]" style={{ background: "var(--surface-raised)" }}>
        {codes.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
      <div className="flex flex-wrap items-center gap-3">
        <CopyButton value={codes.join("\n")} label="Copy all codes" />
        <label className="flex items-center gap-2 text-body cursor-pointer">
          <input type="checkbox" className="h-4 w-4" style={{ accentColor: "var(--accent)" }} checked={saved} onChange={(e) => setSaved(e.target.checked)} />
          I&apos;ve saved my recovery codes
        </label>
      </div>
      {children}
      <button type="button" disabled={!saved} onClick={onDone} className="btn btn-primary self-start">
        {doneLabel}
      </button>
    </div>
  );
}
