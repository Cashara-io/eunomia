"use client";

import { useCallback, useState } from "react";
import { ShieldCheckIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { CodeField, EnrolSteps, RecoveryCodes } from "@/components/mfa/Mfa";
import { useMe } from "@/lib/queries/auth";
import {
  useConfirmEnrolment,
  useDisableMfa,
  useMfaStatus,
  useRegenerateCodes,
  useServerSecurity,
  useSetServerSecurity,
  useStartEnrolment,
} from "@/lib/queries/mfa";
import { ICON, PanelHead, StatusLine } from "./shared";

type Mode = "idle" | "enrol" | "codes" | "regenerate" | "disable";

export function MfaSection() {
  const me = useMe().data;
  const statusQuery = useMfaStatus();
  const status = statusQuery.data;
  const [mode, setMode] = useState<Mode>("idle");
  const [codes, setCodes] = useState<string[] | null>(null);
  const start = useStartEnrolment().mutateAsync;
  const confirm = useConfirmEnrolment().mutateAsync;

  const begin = useCallback(() => start(), [start]);
  const finish = useCallback(async (code: string) => (await confirm(code)).recovery_codes, [confirm]);

  const tone = status?.enabled ? "good" : status?.available === false ? "off" : status?.required ? "warning" : "off";
  const words = !status
    ? "…"
    : status.enabled
      ? `On · ${status.recovery_codes_left} recovery code${status.recovery_codes_left === 1 ? "" : "s"} left`
      : status.available
        ? "Off"
        : "Handled by your sign-in provider";

  return (
    <div className="flex flex-col gap-8">
      <div className="flex flex-col gap-5">
        <PanelHead title="Two-step verification" status={<StatusLine tone={tone}>{words}</StatusLine>}>
          After your password, Eunomia asks for a code from an authenticator app on your phone, so a stolen password alone can&apos;t sign in.
        </PanelHead>
        {statusQuery.isError && <ErrorLine error={failure(statusQuery.error, "Could not load your two-step verification.", " Reload the page to try again.")} />}

        {status && !status.available && (
          <p className="flex items-center gap-2 text-body" style={{ color: "var(--ink-dim)" }}>
            <ShieldCheckIcon {...ICON} aria-hidden />
            You sign in with {me?.oidc_provider ?? "single sign-on"}. Your identity provider handles two-step verification.
          </p>
        )}

        {status?.available && !status.enabled && mode === "idle" && (
          <div className="flex flex-col gap-2 items-start">
            {status.required && (
              <p className="text-body" style={{ color: "var(--warning)" }}>
                Your admin requires two-step verification. You&apos;ll be asked to set it up at your next sign-in.
              </p>
            )}
            <button type="button" className="btn btn-primary" onClick={() => setMode("enrol")}>
              Set up two-step verification
            </button>
          </div>
        )}

        {mode === "enrol" && (
          <EnrolSteps
            start={begin}
            confirm={finish}
            onCodes={(c) => {
              setCodes(c);
              setMode("codes");
            }}
          />
        )}

        {mode === "codes" && codes && (
          <RecoveryCodes
            codes={codes}
            onDone={() => {
              setCodes(null);
              setMode("idle");
            }}
          />
        )}

        {status?.enabled && mode === "idle" && (
          <div className="flex flex-wrap gap-2">
            <button type="button" className="btn btn-ghost" onClick={() => setMode("regenerate")}>
              New recovery codes
            </button>
            {!status.required && (
              <button type="button" className="btn btn-ghost" onClick={() => setMode("disable")}>
                Turn off
              </button>
            )}
          </div>
        )}

        {(mode === "regenerate" || mode === "disable") && (
          <ConfirmWithCode
            what={mode}
            onCancel={() => setMode("idle")}
            onCodes={(c) => {
              setCodes(c);
              setMode("codes");
            }}
            onDone={() => setMode("idle")}
          />
        )}
      </div>

      {me?.is_admin && <RequireMfa />}
    </div>
  );
}

/** New recovery codes, or turning it off: both need a current code (or a recovery code). */
function ConfirmWithCode({
  what,
  onCancel,
  onCodes,
  onDone,
}: {
  what: "regenerate" | "disable";
  onCancel: () => void;
  onCodes: (codes: string[]) => void;
  onDone: () => void;
}) {
  const regenerate = useRegenerateCodes();
  const disable = useDisableMfa();
  const [code, setCode] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (what === "regenerate") onCodes((await regenerate.mutateAsync(code.trim())).recovery_codes);
      else {
        await disable.mutateAsync(code.trim());
        onDone();
      }
    } catch (err) {
      setError(failure(err, "That code didn't work. Try again."));
      setCode("");
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3">
      <p className="text-body" style={{ color: "var(--ink)" }}>
        {what === "regenerate"
          ? "Your old recovery codes stop working. Enter a code from your authenticator app (or a recovery code) to continue."
          : "Signing in will need only your password again. Enter a code from your authenticator app (or a recovery code) to turn it off."}
      </p>
      <CodeField id="mfa-confirm-code" label="Code" recovery value={code} onChange={setCode} invalid={Boolean(error)} />
      {error && <ErrorLine error={error} />}
      <div className="flex gap-2">
        <button type="submit" disabled={busy || !code.trim()} className={what === "disable" ? "btn btn-danger-solid" : "btn btn-primary"}>
          {busy ? "Checking…" : what === "regenerate" ? "Make new codes" : "Turn off two-step verification"}
        </button>
        <button type="button" className="btn btn-ghost" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** The admin's switch: every password account on this server must use two-step verification. */
function RequireMfa() {
  const security = useServerSecurity();
  const set = useSetServerSecurity();
  const s = security.data;
  const [stepUp, setStepUp] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  async function change(on: boolean) {
    setBusy(true);
    setError(null);
    try {
      await set.mutateAsync({ require_mfa: on, ...(stepUp && code.trim() ? { mfa_code: code.trim() } : {}) });
      setStepUp(false);
      setCode("");
    } catch (err) {
      const f = failure(err, "Could not change the setting. Try again.");
      if (f.code === "auth.step_up_required") setStepUp(true);
      else setError(f);
      setCode("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4" data-testid="require-mfa">
      <PanelHead
        title="Require for everyone"
        status={s && <StatusLine tone={s.require_mfa ? "good" : "off"}>{s.require_mfa ? "Required" : "Optional"}</StatusLine>}
      >
        Every account that signs in with a password must use two-step verification. People who sign in with single sign-on are
        left to your identity provider. API tokens and connected apps keep working.
      </PanelHead>
      {security.isError && <ErrorLine error={failure(security.error, "Could not load the server's security settings.")} />}
      {s && (
        <>
          {s.without_mfa > 0 && (
            <p className="text-body" style={{ color: "var(--ink-dim)" }}>
              {s.without_mfa} password account{s.without_mfa === 1 ? " hasn't" : "s haven't"} set it up yet.
              {s.require_mfa ? " They'll be asked at their next sign-in." : " Turning this on signs them out until they do."}
            </p>
          )}
          {stepUp && (
            <>
              <p role="status" className="text-body" style={{ color: "var(--ink-dim)" }}>
                Confirm it&apos;s you: enter a code from your authenticator app (or a recovery code).
              </p>
              <CodeField id="require-mfa-code" label="Code" recovery value={code} onChange={setCode} />
            </>
          )}
          {error && <ErrorLine error={error} />}
          <button
            type="button"
            disabled={busy || (stepUp && !code.trim())}
            className={s.require_mfa ? "btn btn-ghost self-start" : "btn btn-primary self-start"}
            onClick={() => change(!s.require_mfa)}
          >
            {busy ? "Saving…" : s.require_mfa ? "Stop requiring two-step verification" : "Require two-step verification"}
          </button>
        </>
      )}
    </div>
  );
}
