"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";
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
      setBusy(false);
    }
  }

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-3">
          <h1 className="page-title">Sign in to Eunomia</h1>
          <p className="mt-1.5 text-[13.5px]" style={{ color: "var(--ink-dim)" }}>
            {mode === "oidc" ? `Use your ${provider} account.` : "Use the account you made on this Eunomia instance."}
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
                <label htmlFor="password" className={LABEL} style={{ color: "var(--ink-dim)" }}>
                  Password
                </label>
                <div className="relative">
                  <input
                    id="password"
                    type={show ? "text" : "password"}
                    required
                    autoComplete="current-password"
                    className="field h-9 w-full pl-3 pr-14 text-[13.5px]"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="password" />
                </div>
              </div>

              {error && <ErrorLine error={error} />}

              <button type="submit" disabled={busy} aria-busy={busy} className="btn btn-primary mt-1 h-9 w-full">
                {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                {busy ? "Signing in…" : "Sign in"}
              </button>

              <p className="mt-2 text-[13px]" style={{ color: "var(--ink-faint)" }}>
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
