"use client";

// After a provider sign-in whose verified email belongs to an account that has a password. The person
// either proves the password (linked, nothing removed) or takes the account over (its password, API
// tokens, connected apps and sessions are removed first). See docs/deployment.md, single sign-on.

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import Link from "next/link";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { oidcLink, oidcLinkInfo } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import AuthShell, { LABEL, RevealToggle } from "../AuthShell";

export default function LinkPage() {
  const info = useQuery({ queryKey: ["auth", "oidc-link"], queryFn: () => call(oidcLinkInfo()), retry: false });
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmTakeOver, setConfirmTakeOver] = useState(false);
  const [done, setDone] = useState<{ took_over: boolean; next: string } | null>(null);

  async function choose(body: { password?: string; take_over?: boolean }) {
    setBusy(true);
    setError(null);
    try {
      const r = await call(oidcLink({ body }));
      if (r.took_over) setDone(r);
      else window.location.replace(r.next || "/");
    } catch (e) {
      setError(failure(e, "Could not link your sign-in. Try again."));
      setPassword("");
    } finally {
      setBusy(false);
    }
  }

  if (done)
    return (
      <AuthShell>
        <div className="flex flex-col gap-4" data-testid="took-over">
          <h1 className="connect-title">You now hold this account</h1>
          <p className="text-reading" style={{ color: "var(--ink-dim)" }}>
            We removed the account&apos;s password, every API token, every connected app and every other session, so nobody else can use them
            any more. Anything already in the account (spaces, memories, documents) was put there by whoever held it before you. Look through it
            before you rely on it.
          </p>
          <button type="button" className="btn btn-primary min-h-11 w-full" onClick={() => window.location.replace(done.next || "/")}>
            Continue
          </button>
        </div>
      </AuthShell>
    );

  if (info.isError)
    return (
      <AuthShell>
        <div className="flex flex-col gap-3">
          <h1 className="connect-title">This sign-in has expired</h1>
          <p className="text-reading" style={{ color: "var(--ink-dim)" }}>
            Go back to <Link href="/login" className="link">sign in</Link> and continue with your provider again.
          </p>
        </div>
      </AuthShell>
    );
  const data = info.data;
  if (!data) return <AuthShell><div className="h-40" aria-hidden /></AuthShell>;

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-2">
          <h1 className="connect-title">An account already uses {data.email}</h1>
          <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
            {data.provider} confirmed this address is yours, but the Eunomia account with it has a password. If you made that account, type its
            password to connect your {data.provider} sign-in to it.
          </p>
        </div>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void choose({ password });
          }}
        >
          <label htmlFor="password" className={LABEL}>
            The account&apos;s password
          </label>
          <div className="relative">
            <input
              id="password"
              type={show ? "text" : "password"}
              required
              autoComplete="current-password"
              className="field h-11 w-full pl-3 pr-16 text-reading"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="password" />
          </div>
          {error && <ErrorLine error={error} />}
          <button type="submit" disabled={busy} className="btn btn-primary min-h-11 w-full">
            {busy ? "Checking…" : "Connect my sign-in"}
          </button>
        </form>
        <div className="flex flex-col gap-2 pt-2 border-t" style={{ borderColor: "var(--border)" }}>
          <p className="text-body" style={{ color: "var(--ink-dim)" }}>
            Didn&apos;t make it, or don&apos;t know the password? Someone may have registered your address before you. You can take the account
            over: its password, API tokens, connected apps and sessions are removed, then it is yours.
          </p>
          {confirmTakeOver ? (
            <div className="flex flex-col gap-2">
              <p className="text-body" style={{ color: "var(--critical)" }}>
                Anyone who had this account loses access now. What it already holds stays, and was put there by them.
              </p>
              <button type="button" disabled={busy} className="btn btn-danger min-h-11 w-full" onClick={() => void choose({ take_over: true })}>
                {busy ? "Taking over…" : "Take the account over"}
              </button>
            </div>
          ) : (
            <button type="button" className="btn min-h-11 w-full" onClick={() => setConfirmTakeOver(true)}>
              I don&apos;t know the password
            </button>
          )}
        </div>
      </div>
    </AuthShell>
  );
}
