"use client";

// After a provider sign-in whose verified email belongs to an account that has a password. The person
// either proves the password (linked, nothing removed) or starts a new account: the existing one is set
// aside for a server operator, and nothing of it is inherited. See docs/deployment.md, single sign-on.

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import Link from "next/link";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { oidcLink, oidcLinkInfo } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import AuthShell, { LABEL, RevealToggle } from "../AuthShell";
import SecondStep from "../SecondStep";

export default function LinkPage() {
  const info = useQuery({ queryKey: ["auth", "oidc-link"], queryFn: () => call(oidcLinkInfo()), retry: false });
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmFresh, setConfirmFresh] = useState(false);
  const [done, setDone] = useState<{ started_fresh: boolean; next: string } | null>(null);
  // the account has its own authenticator (or this server requires one): the password opened its second step,
  // and the link is made when that step passes
  const [second, setSecond] = useState<{ step: "verify" | "enrol"; next: string } | null>(null);

  async function choose(body: { password?: string; start_fresh?: boolean }) {
    setBusy(true);
    setError(null);
    try {
      const r = await call(oidcLink({ body }));
      if (r.mfa === "verify" || r.mfa === "enrol") setSecond({ step: r.mfa, next: r.next || "/" });
      else if (r.started_fresh) setDone(r);
      else window.location.replace(r.next || "/");
    } catch (e) {
      setError(failure(e, "Could not link your sign-in. Try again."));
      setPassword("");
    } finally {
      setBusy(false);
    }
  }

  if (second)
    return (
      <AuthShell>
        <SecondStep
          step={second.step}
          onDone={() => window.location.replace(second.next)}
          onRestart={(why) => {
            setSecond(null);
            setError(why);
          }}
        />
      </AuthShell>
    );

  if (done)
    return (
      <AuthShell>
        <div className="flex flex-col gap-4" data-testid="started-fresh">
          <h1 className="connect-title">Your new account is ready</h1>
          <p className="text-reading" style={{ color: "var(--ink-dim)" }}>
            It starts empty. The account that used this address before is set aside: nobody can sign in to it or use its tokens and
            connections any more, and its data is kept for a server operator, who can restore it if it was yours.
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
            {data.provider} confirmed this address is yours. If the existing account is yours, enter its password to connect your{" "}
            {data.provider} sign-in to it and keep everything.
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
            If you don&apos;t know the password, you can start a new account instead. The old one is set aside, not deleted: nobody can use it,
            and a server operator can restore it. If it was your own account, your data is not lost, but getting it back needs an operator,
            and every AI app, agent and API token you connected will need connecting again.
          </p>
          {confirmFresh ? (
            <div className="flex flex-col gap-2">
              <p className="text-body" style={{ color: "var(--critical)" }}>
                The existing account will be set aside and you&apos;ll start with an empty one.
              </p>
              <button type="button" disabled={busy} className="btn btn-danger min-h-11 w-full" onClick={() => void choose({ start_fresh: true })}>
                {busy ? "Starting…" : "Start a new account"}
              </button>
            </div>
          ) : (
            <button type="button" className="btn min-h-11 w-full" onClick={() => setConfirmFresh(true)}>
              I don&apos;t know the password
            </button>
          )}
        </div>
      </div>
    </AuthShell>
  );
}
