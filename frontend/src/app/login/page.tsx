"use client";

import { useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { ArrowRight, Info, Loader2 } from "lucide-react";
import { useLogin } from "@/lib/queries/auth";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { keepNext, nextPath } from "@/lib/nextPath";
import AuthShell, { LABEL, RevealToggle } from "./AuthShell";
import { OrDivider, SsoButton, useOidcError, useSignIn } from "./Sso";
import SecondStep from "./SecondStep";

const noSubscribe = () => () => {};
/** `?email=` from "Sign in instead" on the register page. */
const emailFromUrl = () => new URLSearchParams(window.location.search).get("email") ?? "";
/** `?erased=1` after Settings > Data > Delete my account. */
const erasedFromUrl = () => new URLSearchParams(window.location.search).get("erased") === "1";
/** `?mfa=verify` from single sign-on into an account with its own authenticator: the second step is waiting. */
const pendingFromUrl = () => (new URLSearchParams(window.location.search).get("mfa") === "verify" ? "verify" : "");

export default function LoginPage() {
  const router = useRouter();
  const login = useLogin();
  const { pending, mode, provider } = useSignIn();
  const oidcError = useOidcError(provider);
  // null until typed in: the field then shows the email the link carried, if any.
  const [typedEmail, setEmail] = useState<string | null>(null);
  const prefill = useSyncExternalStore(noSubscribe, emailFromUrl, () => "");
  const erased = useSyncExternalStore(noSubscribe, erasedFromUrl, () => false);
  const email = typedEmail ?? prefill;
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  // a correct password that still needs its second step (a code, or setting up an authenticator first)
  const [typedSecond, setSecond] = useState<"verify" | "enrol" | null | undefined>(undefined);
  const fromSso = useSyncExternalStore(noSubscribe, pendingFromUrl, () => "");
  // the URL's pending step counts until the person goes back to the password (null)
  const second = typedSecond === undefined ? (fromSso === "verify" ? "verify" : null) : typedSecond;
  // a suspended account is not a mistake the person made: it reads as a calm notice, and the fields stay valid
  const suspended = error?.code === "auth.suspended";
  const invalid = error && !suspended ? true : undefined;

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const r = await login.mutateAsync({ email, password });
      setPassword("");
      if (r.kind === "mfa") {
        setSecond(r.step);
        setBusy(false);
        return;
      }
      router.replace(nextPath());
    } catch (err) {
      setError(failure(err, "Could not sign in. Check your email and password, then try again."));
      // Keep the email; clear only the password, so nothing secret lingers on screen.
      setPassword("");
      setBusy(false);
    }
  }

  if (second) {
    return (
      <AuthShell>
        <SecondStep
          step={second}
          onDone={() => router.replace(nextPath())}
          onRestart={(why) => {
            setSecond(null);
            setError(why);
          }}
        />
      </AuthShell>
    );
  }

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-3">
          <h1 className="connect-title">Sign in to Eunomia</h1>
          <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
            {mode === "oidc" ? `Use your ${provider} account.` : "Use your Eunomia email and password."}
          </p>
        </div>

        {erased && (
          <p role="status" className="text-reading flex items-start gap-2" data-testid="account-erased">
            <Info size={16} className="mt-1 shrink-0" aria-hidden />
            Your account is being deleted. You have been signed out, and your data is removed in the next few minutes.
          </p>
        )}
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
                  autoFocus={!provider && !prefill}
                  autoComplete="email"
                  spellCheck={false}
                  className="field h-11 px-3 text-reading"
                  placeholder="you@example.com"
                  aria-invalid={invalid}
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
                    autoFocus={!provider && Boolean(prefill)}
                    autoComplete="current-password"
                    className="field h-11 w-full pl-3 pr-16 text-reading"
                    aria-invalid={invalid}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="password" />
                </div>
              </div>

              {error && (suspended ? <SuspendedNotice message={error.message} /> : <ErrorLine error={error} />)}

              <button type="submit" disabled={busy} aria-busy={busy} className="btn btn-primary mt-1 min-h-11 w-full">
                {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                {busy ? "Signing in…" : "Sign in"}
                {!busy && <ArrowRight size={16} strokeWidth={1.75} aria-hidden />}
              </button>

              <p className="mt-2 text-body" style={{ color: "var(--ink-dim)" }}>
                No account yet?{" "}
                <Link href="/register" onClick={(e) => { e.preventDefault(); router.push(keepNext("/register")); }} className="link font-medium">
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

/** The sign-in answer for a suspended account: the server's words, no code or trace, no alarm. */
function SuspendedNotice({ message }: { message: string }) {
  return (
    <div role="status" className="flex items-start gap-2.5 px-3.5 py-2.5 rounded-[10px]" style={{ background: "var(--surface-raised)" }}>
      <Info size={18} strokeWidth={1.75} aria-hidden className="shrink-0 mt-px" style={{ color: "var(--ink-dim)" }} />
      <p className="text-body leading-snug" style={{ color: "var(--ink)" }}>
        {message}
      </p>
    </div>
  );
}
