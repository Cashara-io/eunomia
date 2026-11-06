"use client";

import { ArrowRightIcon, CircleNotchIcon } from "@phosphor-icons/react/ssr";
import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useLogin } from "@/lib/queries/auth";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { keepNext, nextPath } from "@/lib/nextPath";
import AuthShell, { LABEL, RevealToggle } from "./AuthShell";
import { OrDivider, SsoButton, useOidcError, useSignIn } from "./Sso";

export default function LoginPage() {
  const router = useRouter();
  const login = useLogin();
  const { pending, mode, provider } = useSignIn();
  const oidcError = useOidcError(provider);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await login.mutateAsync({ email, password });
      router.replace(nextPath());
    } catch (err) {
      setError(failure(err, "Could not sign in. Check your email and password, then try again."));
      // Keep the email; clear only the password, so nothing secret lingers on screen.
      setPassword("");
      setBusy(false);
    }
  }

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-3">
          <h1 className="connect-title">Sign in to Eunomia</h1>
          <p className="mt-3 text-[16px]" style={{ color: "var(--ink-dim)" }}>
            {mode === "oidc" ? `Use your ${provider} account.` : "Use your Eunomia email and password."}
          </p>
        </div>

        {oidcError && <ErrorLine error={oidcError} />}

        {pending ? (
          <div className="h-10" aria-hidden />
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
                <label htmlFor="email" className={LABEL}>
                  Email
                </label>
                <input
                  id="email"
                  type="email"
                  required
                  autoFocus={!provider}
                  autoComplete="email"
                  spellCheck={false}
                  className="field h-11 px-3 text-[15px]"
                  placeholder="you@example.com"
                  aria-invalid={error ? true : undefined}
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                />
              </div>

              <div className="flex flex-col gap-1.5">
                <label htmlFor="password" className={LABEL}>
                  Password
                </label>
                <div className="relative">
                  <input
                    id="password"
                    type={show ? "text" : "password"}
                    required
                    autoComplete="current-password"
                    className="field h-11 w-full pl-3 pr-16 text-[15px]"
                    aria-invalid={error ? true : undefined}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="password" />
                </div>
              </div>

              {error && <ErrorLine error={error} />}

              <button type="submit" disabled={busy} aria-busy={busy} className="btn btn-primary mt-1 min-h-11 w-full">
                {busy && <CircleNotchIcon weight="duotone" size={14} className="animate-spin" aria-hidden />}
                {busy ? "Signing in…" : "Sign in"}
                {!busy && <ArrowRightIcon weight="duotone" size={16} aria-hidden />}
              </button>

              <p className="mt-2 text-[14px]" style={{ color: "var(--ink-dim)" }}>
                No account yet?{" "}
                <Link href="/register" onClick={(e) => { e.preventDefault(); router.push(keepNext("/register")); }} className="font-medium underline" style={{ color: "var(--accent-text)" }}>
                  Create one
                </Link>
              </p>
            </form>
          </>
        )}
      </div>
    </AuthShell>
  );
}
