"use client";

// The page a password reset link opens: `/reset-password#<token>`. The token rides in the fragment, which the
// browser never sends to a server; it is taken into this page's memory and the address bar is cleared, so it
// is not left in history. Resetting ends every session of the account and starts none: the person signs in
// with the new password (and their second factor, if they use one).

import { useEffect, useState } from "react";
import Link from "next/link";
import { ArrowRight, CheckCircle2, Loader2 } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useResetPassword } from "@/lib/queries/recovery";
import AuthShell, { LABEL, RevealToggle } from "../login/AuthShell";

const MISMATCH = "Passwords don't match.";

/** Plain words for the refusals a link can meet. Password rule messages come from the server already plain. */
const PLAIN: Record<string, string> = {
  "auth.reset_invalid": "This reset link isn't valid any more. It may have been used already, or a newer one was sent. Ask for a new one.",
  "auth.reset_expired": "This reset link has expired. They last 30 minutes. Ask for a new one.",
  "rate.limited": "Too many attempts. Wait a few minutes, then try again.",
  "auth.password_disabled": "This server signs people in with a company account, not a password. There is no password to reset.",
};

export default function ResetPasswordPage() {
  const reset = useResetPassword();
  // the fragment only exists in the browser: read once after mount, then clear it from the address bar
  const [token, setToken] = useState<string | null>(null);
  const [read, setRead] = useState(false);
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [done, setDone] = useState(false);

  useEffect(() => {
    const t = window.location.hash.replace(/^#/, "").trim();
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the fragment is not readable on the server
    setToken(t || null);
    setRead(true);
    if (window.location.hash) window.history.replaceState(window.history.state, "", window.location.pathname);
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    if (password !== confirm) {
      setError({ message: MISMATCH });
      return;
    }
    try {
      await reset.mutateAsync({ token: token ?? "", password });
      setPassword("");
      setConfirm("");
      setDone(true);
    } catch (err) {
      const f = failure(err, "Could not set the password. Try again.");
      setError(f.code && PLAIN[f.code] ? { ...f, message: PLAIN[f.code] } : f);
    }
  }

  if (!read) {
    return (
      <AuthShell>
        <div className="h-40" aria-hidden />
      </AuthShell>
    );
  }

  if (done) {
    return (
      <AuthShell>
        <div className="flex flex-col gap-4" data-testid="reset-done">
          <h1 className="connect-title">Password changed</h1>
          <p role="status" className="text-reading flex items-start gap-2" style={{ color: "var(--ink-dim)" }}>
            <CheckCircle2 size={16} className="mt-1 shrink-0" aria-hidden />
            You&apos;ve been signed out everywhere. Sign in with your new password. If you use two-step verification, you&apos;ll still be asked for a
            code.
          </p>
          <Link href="/login" className="btn btn-primary min-h-11 w-full">
            Sign in
            <ArrowRight size={16} strokeWidth={1.75} aria-hidden />
          </Link>
        </div>
      </AuthShell>
    );
  }

  const linkProblem = error?.code === "auth.reset_invalid" || error?.code === "auth.reset_expired";
  if (!token || linkProblem) {
    return (
      <AuthShell>
        <div className="flex flex-col gap-4" data-testid="reset-link-problem">
          <h1 className="connect-title">This link can&apos;t be used</h1>
          <p className="text-reading" style={{ color: "var(--ink-dim)" }}>
            {error?.code && PLAIN[error.code] ? PLAIN[error.code] : "This page needs the link from your reset email. Open it from the email, or ask for a new one."}
          </p>
          <Link href="/forgot-password" className="btn btn-primary min-h-11 w-full">
            Ask for a new link
          </Link>
        </div>
      </AuthShell>
    );
  }

  return (
    <AuthShell>
      <form onSubmit={submit} className="flex flex-col gap-4">
        <div className="mb-3">
          <h1 className="connect-title">Choose a new password</h1>
          <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
            At least 8 characters. You&apos;ll be signed out everywhere, then sign in with the new one.
          </p>
        </div>

        <div className="flex flex-col gap-1.5">
          <label htmlFor="new-password" className={LABEL}>
            New password
          </label>
          <div className="relative">
            <input
              id="new-password"
              type={show ? "text" : "password"}
              required
              autoFocus
              autoComplete="new-password"
              className="field h-11 w-full pl-3 pr-16 text-reading"
              aria-invalid={error ? true : undefined}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="new-password" />
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <label htmlFor="confirm-password" className={LABEL}>
            Confirm new password
          </label>
          <input
            id="confirm-password"
            type={show ? "text" : "password"}
            required
            autoComplete="new-password"
            className="field h-11 px-3 text-reading"
            aria-invalid={error?.message === MISMATCH ? true : undefined}
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </div>

        {error && <ErrorLine error={error} />}

        <button type="submit" disabled={reset.isPending} aria-busy={reset.isPending} className="btn btn-primary mt-1 min-h-11 w-full">
          {reset.isPending && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
          {reset.isPending ? "Saving…" : "Set new password"}
          {!reset.isPending && <ArrowRight size={16} strokeWidth={1.75} aria-hidden />}
        </button>
      </form>
    </AuthShell>
  );
}
