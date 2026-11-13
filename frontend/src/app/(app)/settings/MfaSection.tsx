"use client";

import { useCallback, useState } from "react";
import { ShieldCheckIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { CodeField, EnrolSteps, RecoveryCodes, type Enrolment } from "@/components/mfa/Mfa";
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

type Mode = "idle" | "password" | "enrol" | "codes" | "regenerate" | "disable";

export function MfaSection() {
  const me = useMe().data;
  const statusQuery = useMfaStatus();
  const status = statusQuery.data;
  const [mode, setMode] = useState<Mode>("idle");
  const [codes, setCodes] = useState<string[] | null>(null);
  // setup starts with the account password, so a stolen session alone can't add an authenticator
  const [password, setPassword] = useState("");
  const [passwordError, setPasswordError] = useState<Failure | null>(null);
  const [enrolment, setEnrolment] = useState<Enrolment | null>(null);
  const start = useStartEnrolment().mutateAsync;
  const confirm = useConfirmEnrolment().mutateAsync;

  async function checkPassword(e: React.FormEvent) {
    e.preventDefault();
    setPasswordError(null);
    try {
      setEnrolment(await start(password));
      setPassword("");
      setMode("enrol");
    } catch (err) {
      setPasswordError(failure(err, "Could not start the setup. Try again."));
      setPassword("");
    }
  }

  const begin = useCallback(async () => {
    if (!enrolment) throw new Error("Start again: enter your password first.");
    return enrolment;
  }, [enrolment]);
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
            <button type="button" className="btn btn-primary" onClick={() => setMode("password")}>
              Set up two-step verification
            </button>
          </div>
        )}

        {mode === "password" && (
          <form onSubmit={checkPassword} className="flex flex-col gap-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="mfa-password" className="text-[14px] font-semibold">
                Your password
              </label>
              <input
                id="mfa-password"
                type="password"
                autoComplete="current-password"
                autoFocus
                required
                className="field h-11 px-3 text-reading w-full max-w-[320px]"
                aria-invalid={passwordError ? true : undefined}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                Confirm it&apos;s you before adding an authenticator.
              </p>
            </div>
            {passwordError && <ErrorLine error={passwordError} />}
            <div className="flex gap-2">
              <button type="submit" disabled={!password} className="btn btn-primary">
                Continue
              </button>
              <button type="button" className="btn btn-ghost" onClick={() => { setMode("idle"); setPassword(""); setPasswordError(null); }}>
                Cancel
              </button>
            </div>
          </form>
        )}

        {mode === "enrol" && (
          <EnrolSteps
            start={begin}
            confirm={finish}
            onCodes={(c) => {
              setCodes(c);
              setEnrolment(null);
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

        {status?.enabled && status.unreadable && mode === "idle" && (
          <p className="text-body" style={{ color: "var(--warning)" }}>
            Your authenticator can&apos;t be checked any more because this server&apos;s encryption key changed. Sign-in works with a recovery
            code. Turn two-step verification off with a recovery code, then set it up again.
          </p>
        )}

        {status?.enabled && mode === "idle" && (
          <div className="flex flex-wrap gap-2">
            <button type="button" className="btn btn-ghost" onClick={() => setMode("regenerate")}>
              New recovery codes
            </button>
            {(!status.required || status.unreadable) && (
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

      {status?.can_require && <RequireMfa />}
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
        Every account on this server, in every organisation, that signs in with a password must use two-step verification.
        People who sign in with single sign-on are left to your identity provider unless they set up their own authenticator.
        API tokens and connected apps keep working. Only the server&apos;s operator sees this.
      </PanelHead>
      {security.isError && <ErrorLine error={failure(security.error, "Could not load the server's security settings.")} />}
      {s && (
        <>
          {s.without_mfa > 0 && (
            <p className="text-body" style={{ color: "var(--ink-dim)" }}>
              {s.without_mfa} password account{s.without_mfa === 1 ? " on this server hasn't" : "s on this server haven't"} set it up yet.
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
