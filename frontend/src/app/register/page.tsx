"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";
import { useRegister } from "@/lib/queries/auth";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { keepNext } from "@/lib/nextPath";
import AuthShell, { LABEL, RevealToggle } from "../login/AuthShell";
import { OrDivider, SsoButton, useOidcError, useSignIn } from "../login/Sso";

const MISMATCH = "Passwords don't match.";

export default function RegisterPage() {
  const router = useRouter();
  const register = useRegister();
  const { pending, mode, provider } = useSignIn();
  const oidcError = useOidcError(provider);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (password !== confirmPassword) {
      setError({ message: MISMATCH });
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await register.mutateAsync({ email, password });
      router.replace(keepNext("/onboarding"));
    } catch (err) {
      setError(failure(err, "Could not create the account. Try again."));
      setBusy(false);
    }
  }

  const mismatch = error?.message === MISMATCH;

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-3">
          <h1 className="page-title">Create your account</h1>
          <p className="mt-1.5 text-[13.5px]" style={{ color: "var(--ink-dim)" }}>
            {mode === "oidc" ? `Sign in with your ${provider} account and Eunomia creates yours.` : "One account for the web app. Your agents connect with tokens later."}
          </p>
        </div>

        {oidcError && <ErrorLine error={oidcError} />}

        {pending ? (
          <div className="h-9" aria-hidden />
        ) : mode === "oidc" ? (
          <SsoButton provider={provider ?? ""} primary />
        ) : (
          <>
            {provider && (
              <>
                <SsoButton provider={provider} primary={false} />
                <OrDivider />
              </>
            )}
            <form onSubmit={submit} className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5">
                <label htmlFor="email" className={LABEL} style={{ color: "var(--ink-dim)" }}>
                  Email
                </label>
                <input
                  id="email"
                  type="email"
                  required
                  autoFocus={!provider}
                  autoComplete="email"
                  spellCheck={false}
                  className="field h-9 px-3 text-[13.5px]"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
              </div>

              <div className="flex flex-col gap-1.5">
                <div className="flex items-baseline justify-between">
                  <label htmlFor="password" className={LABEL} style={{ color: "var(--ink-dim)" }}>
                    Password
                  </label>
                  <span id="password-hint" className="label">
                    8 to 72 characters
                  </span>
                </div>
                <div className="relative">
                  <input
                    id="password"
                    type={show ? "text" : "password"}
                    required
                    minLength={8}
                    maxLength={72}
                    autoComplete="new-password"
                    aria-describedby="password-hint"
                    className="field h-9 w-full pl-3 pr-14 text-[13.5px]"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="password confirm-password" />
                </div>
              </div>

              <div className="flex flex-col gap-1.5">
                <label htmlFor="confirm-password" className={LABEL} style={{ color: "var(--ink-dim)" }}>
                  Confirm password
                </label>
                <input
                  id="confirm-password"
                  type={show ? "text" : "password"}
                  required
                  minLength={8}
                  maxLength={72}
                  autoComplete="new-password"
                  aria-invalid={mismatch || undefined}
                  className="field h-9 px-3 text-[13.5px]"
                  style={mismatch ? { borderColor: "var(--critical)" } : undefined}
                  value={confirmPassword}
                  onChange={(e) => {
                    setConfirmPassword(e.target.value);
                    if (mismatch) setError(null);
                  }}
                />
              </div>

              {error && <ErrorLine error={error} />}

              <button type="submit" disabled={busy} aria-busy={busy} className="btn btn-primary mt-1 h-9 w-full">
                {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                {busy ? "Creating account…" : "Create account"}
              </button>

              <p className="mt-2 text-[13px]" style={{ color: "var(--ink-faint)" }}>
                Already have an account?{" "}
                <Link href="/login" onClick={(e) => { e.preventDefault(); router.push(keepNext("/login")); }} className="font-medium underline" style={{ color: "var(--accent-text)" }}>
                  Sign in
                </Link>
              </p>
            </form>
          </>
        )}
      </div>
    </AuthShell>
  );
}
