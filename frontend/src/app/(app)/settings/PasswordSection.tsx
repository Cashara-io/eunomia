"use client";

import { useState } from "react";
import { KeyIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useMe } from "@/lib/queries/auth";
import { useChangePassword } from "@/lib/queries/recovery";
import { ICON, PanelHead } from "./shared";

/** Settings > Security: change your own password. Other sessions end, this one stays. An account that only
 * signs in through single sign-on has no password, so it only says so. */
export function PasswordSection() {
  const me = useMe().data;
  const change = useChangePassword();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [done, setDone] = useState(false);
  // two-step verification on and its last second factor is older than 10 minutes: a code now
  const [stepUp, setStepUp] = useState(false);
  const [code, setCode] = useState("");

  if (!me) return null;
  if (!me.has_password) {
    return (
      <div className="flex flex-col gap-3">
        <PanelHead title="Password">You sign in with {me.oidc_provider ?? "single sign-on"}, so this account has no password to change.</PanelHead>
      </div>
    );
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setDone(false);
    if (next !== confirm) {
      setError({ message: "The new passwords don't match." });
      return;
    }
    try {
      await change.mutateAsync({ current_password: current, new_password: next, ...(stepUp && code.trim() ? { mfa_code: code.trim() } : {}) });
      setCurrent("");
      setNext("");
      setConfirm("");
      setCode("");
      setStepUp(false);
      setDone(true);
    } catch (err) {
      const f = failure(err, "Your password was not changed.");
      if (f.code === "auth.step_up_required") {
        setStepUp(true);
        return;
      }
      setCurrent("");
      setError(f);
    }
  }

  return (
    <div className="flex flex-col gap-5" data-testid="change-password">
      <PanelHead title="Password">Changing it signs you out of every other browser and device. This one stays signed in.</PanelHead>
      <form onSubmit={submit} className="flex flex-col gap-3 max-w-md">
        <div className="flex flex-col gap-1">
          <label htmlFor="current-password" className="label">
            Current password
          </label>
          <input
            id="current-password"
            type="password"
            required
            autoComplete="current-password"
            className="field h-10 px-3 text-[14px]"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor="change-new-password" className="label">
            New password
          </label>
          <input
            id="change-new-password"
            type="password"
            required
            autoComplete="new-password"
            className="field h-10 px-3 text-[14px]"
            value={next}
            onChange={(e) => setNext(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor="change-confirm-password" className="label">
            Confirm new password
          </label>
          <input
            id="change-confirm-password"
            type="password"
            required
            autoComplete="new-password"
            className="field h-10 px-3 text-[14px]"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </div>
        {stepUp && (
          <div className="flex flex-col gap-1">
            <label htmlFor="change-mfa-code" className="label">
              Authenticator or recovery code
            </label>
            <input
              id="change-mfa-code"
              inputMode="text"
              autoComplete="one-time-code"
              required
              className="field h-10 px-3 text-[14px]"
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
            <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
              Changing your password needs a code from the last 10 minutes.
            </span>
          </div>
        )}
        {error && <ErrorLine error={error} />}
        {done && (
          <p role="status" className="flex items-center gap-2 text-[13px]" data-testid="password-changed">
            <KeyIcon {...ICON} aria-hidden />
            Password changed. Your other sessions were signed out.
          </p>
        )}
        <button type="submit" className="btn btn-primary self-start" disabled={change.isPending}>
          {change.isPending ? "Changing…" : "Change password"}
        </button>
      </form>
    </div>
  );
}
