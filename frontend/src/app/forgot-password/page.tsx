"use client";

// Ask for a password reset link. The server answers the same for every well-formed address, whether or not an
// account exists, so this page says only that a link was sent *if* there is an account for the address.

import { useState } from "react";
import Link from "next/link";
import { ArrowRight, Info, Loader2 } from "lucide-react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useForgotPassword } from "@/lib/queries/recovery";
import AuthShell, { LABEL } from "../login/AuthShell";
import { useSignIn } from "../login/Sso";

const PLAIN: Record<string, string> = {
  "rate.limited": "Too many requests. Wait a few minutes, then try again.",
  "email.unavailable": "This server doesn't send email, so it can't send a reset link. Ask whoever runs it to reset your password.",
  "auth.password_disabled": "This server signs people in with a company account, not a password. There is no password to reset.",
};

export default function ForgotPasswordPage() {
  const forgot = useForgotPassword();
  const { pending, mode, emailEnabled } = useSignIn();
  const [email, setEmail] = useState("");
  const [error, setError] = useState<Failure | null>(null);
  const [sentTo, setSentTo] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await forgot.mutateAsync(email.trim());
      setSentTo(email.trim());
    } catch (err) {
      const f = failure(err, "Could not send the link. Try again in a moment.");
      setError(f.code && PLAIN[f.code] ? { ...f, message: PLAIN[f.code] } : f);
    }
  }

  const unavailable = !pending && (!emailEnabled || mode === "oidc");

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-3">
          <h1 className="connect-title">Reset your password</h1>
          <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
            Enter the email you sign in with. If there is an account for it, we&apos;ll send a link to choose a new password.
          </p>
        </div>

        {sentTo ? (
          <div role="status" className="flex items-start gap-2.5 px-3.5 py-2.5 rounded-[10px]" style={{ background: "var(--surface-raised)" }} data-testid="forgot-sent">
            <Info size={18} strokeWidth={1.75} aria-hidden className="shrink-0 mt-px" style={{ color: "var(--ink-dim)" }} />
            <p className="text-body leading-snug">
              If there&apos;s an account for <strong>{sentTo}</strong>, a link is on its way. It works once and expires in 30 minutes. Check your spam
              folder if it doesn&apos;t arrive.
            </p>
          </div>
        ) : unavailable ? (
          <p role="status" className="flex items-start gap-2 text-reading" data-testid="forgot-unavailable">
            <Info size={16} className="mt-1 shrink-0" aria-hidden />
            {mode === "oidc"
              ? PLAIN["auth.password_disabled"]
              : "This server doesn't send email, so it can't send a reset link. Ask whoever runs it to reset your password."}
          </p>
        ) : (
          <form onSubmit={submit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="email" className={LABEL}>
                Email
              </label>
              <input
                id="email"
                type="email"
                required
                autoFocus
                autoComplete="email"
                spellCheck={false}
                className="field h-11 px-3 text-reading"
                placeholder="you@example.com"
                aria-invalid={error ? true : undefined}
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </div>
            {error && <ErrorLine error={error} />}
            <button type="submit" disabled={forgot.isPending} aria-busy={forgot.isPending} className="btn btn-primary mt-1 min-h-11 w-full">
              {forgot.isPending && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
              {forgot.isPending ? "Sending…" : "Send reset link"}
              {!forgot.isPending && <ArrowRight size={16} strokeWidth={1.75} aria-hidden />}
            </button>
          </form>
        )}

        <p className="mt-2 text-body" style={{ color: "var(--ink-dim)" }}>
          <Link href="/login" className="link font-medium">
            Back to sign in
          </Link>
        </p>
      </div>
    </AuthShell>
  );
}
