"use client";

// The page an organisation invitation link opens: `/invite#<token>`. The token rides in the fragment,
// which the browser never sends to a server, and is kept in this tab's sessionStorage so it survives a
// detour through sign-in or single sign-on (both come back to `/invite`).

import { useEffect, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { CircleNotchIcon, KeyIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useLogout, useMe, useRegister } from "@/lib/queries/auth";
import { useAcceptInvitation, useInvitationLookup } from "@/lib/queries/org";
import { ApiError } from "@/lib/api";
import AuthShell, { LABEL, RevealToggle } from "../login/AuthShell";
import { OrDivider, useSignIn } from "../login/Sso";

const KEY = "eunomia_invite";
const noSubscribe = () => () => {};

/** The token from the fragment (moved into sessionStorage, and the fragment cleared), or the one kept earlier. */
function readToken(): string | null {
  const fromHash = window.location.hash.replace(/^#/, "").trim();
  if (fromHash) {
    sessionStorage.setItem(KEY, fromHash);
    return fromHash;
  }
  return sessionStorage.getItem(KEY);
}

/** True when this visit came back to the page (after a sign-in) rather than from the link itself. */
const cameBack = () => !window.location.hash && !!sessionStorage.getItem(KEY);

/** Plain words for the refusals a link can meet. */
const PLAIN: Record<string, string> = {
  "invitation.not_found": "This invitation link isn't valid any more. It may have been used or withdrawn. Ask for a new one.",
  "rate.limited": "Too many attempts. Wait a few minutes, then try again.",
};

function Message({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <AuthShell>
      <div className="flex flex-col gap-3">
        <h1 className="connect-title">{title}</h1>
        <div className="text-reading flex flex-col gap-3" style={{ color: "var(--ink-dim)" }}>
          {children}
        </div>
      </div>
    </AuthShell>
  );
}

export default function InvitePage() {
  const router = useRouter();
  // the fragment only exists in the browser
  const [returning] = useState(() => typeof window !== "undefined" && cameBack());
  const [token] = useState<string | null>(() => (typeof window === "undefined" ? null : readToken()));
  const hydrated = useSyncExternalStore(noSubscribe, () => true, () => false);
  const lookup = useInvitationLookup(token);
  const me = useMe(!!token);
  const accept = useAcceptInvitation();
  const register = useRegister();
  const logout = useLogout();
  const { mode, provider } = useSignIn();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [show, setShow] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [joined, setJoined] = useState<string | null>(null);
  // nothing is created or joined until the person has seen the organisation and the role and said yes
  const [confirmed, setConfirmed] = useState(false);

  // the token is kept in this tab only: take it out of the address bar (and so out of history and bookmarks)
  useEffect(() => {
    if (window.location.hash) window.history.replaceState(window.history.state, "", window.location.pathname);
  }, []);

  // once used, the link is forgotten
  useEffect(() => {
    if (joined) sessionStorage.removeItem(KEY);
  }, [joined]);

  if (!hydrated) return <AuthShell><div className="h-40" aria-hidden /></AuthShell>;
  if (!token)
    return (
      <Message title="No invitation here">
        <p>Open the invitation link you were sent. It ends with a long code after a # sign.</p>
      </Message>
    );
  if (lookup.isError) {
    const code = lookup.error instanceof ApiError ? lookup.error.code : "";
    // back from a sign-in that used this link up (single sign-on joins as it signs in): that is success
    if (returning && code === "invitation.not_found" && me.data) {
      sessionStorage.removeItem(KEY);
      return (
        <Message title="You're signed in">
          <p data-testid="invite-done">If you just joined through this invitation, you&apos;re all set.</p>
          <Link href="/" className="btn btn-primary min-h-11 w-full">
            Go to Eunomia
          </Link>
        </Message>
      );
    }
    return (
      <Message title="This link doesn't work">
        <p>{PLAIN[code] ?? "Could not open this invitation. Try the link again."}</p>
      </Message>
    );
  }
  const inv = lookup.data;
  if (!inv || me.isPending)
    return (
      <AuthShell>
        <div className="flex items-center gap-2" style={{ color: "var(--ink-dim)" }}>
          <CircleNotchIcon size={16} className="animate-spin" aria-hidden /> Opening the invitation…
        </div>
      </AuthShell>
    );

  const org = inv.org_name || "this organisation";
  const roleWords = inv.role === "admin" ? "an admin (you can invite members)" : "a member";
  const as = ` as ${roleWords}`;
  if (joined)
    return (
      <Message title={`You're in ${joined}`}>
        <Link href="/" className="btn btn-primary min-h-11 w-full" data-testid="invite-continue">
          Go to Eunomia
        </Link>
      </Message>
    );
  if (inv.status === "expired")
    return (
      <Message title="This invitation has expired">
        <p>
          Ask an admin of {org} to send a new link. Invitations last 7 days.
        </p>
      </Message>
    );

  const signedIn = me.data;
  if (signedIn) {
    const same = signedIn.email.trim().toLowerCase() === inv.email;
    return (
      <Message title={`Join ${org}`}>
        {same ? (
          <>
            <p>
              You&apos;re signed in as <strong>{signedIn.email}</strong>. You were invited to {org}
              {as}.
            </p>
            {error && <ErrorLine error={error} />}
            <button
              type="button"
              className="btn btn-primary min-h-11 w-full"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                setError(null);
                try {
                  const r = await accept.mutateAsync(token);
                  setJoined(r.org_name || org);
                } catch (e) {
                  setError(failure(e, "Could not accept the invitation."));
                } finally {
                  setBusy(false);
                }
              }}
            >
              {busy ? "Joining…" : `Join ${org} as ${inv.role === "admin" ? "an admin" : "a member"}`}
            </button>
          </>
        ) : (
          <>
            <p>
              This invitation is for <strong>{inv.email}</strong>, but you&apos;re signed in as <strong>{signedIn.email}</strong>.
            </p>
            <button type="button" className="btn min-h-11 w-full" onClick={() => logout.mutate(undefined, { onSettled: () => window.location.reload() })}>
              Sign out and use {inv.email}
            </button>
          </>
        )}
      </Message>
    );
  }

  async function signUp(e: React.FormEvent) {
    e.preventDefault();
    if (password !== confirm) {
      setError({ message: "Passwords don't match." });
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await register.mutateAsync({ email: inv!.email, password, invitation: token! });
      sessionStorage.removeItem(KEY);
      router.replace("/onboarding");
    } catch (err) {
      const f = failure(err, "Could not create the account. Try again.");
      setError(f.code === "auth.email_taken" ? { message: "There's already an account for this email. Sign in instead, then come back to this link." } : f);
      setBusy(false);
    }
  }

  const signIn = `/login?next=${encodeURIComponent("/invite")}&email=${encodeURIComponent(inv.email)}`;

  if (!confirmed)
    return (
      <Message title={`Join ${org}?`}>
        <p data-testid="invite-summary">
          An admin of {org} invited{" "}
          <strong>{inv.email}</strong> to join <strong>{org}</strong> as {roleWords}. Everything you save goes to {org}, and for now an
          account can only be in one organisation.
        </p>
        <button type="button" className="btn btn-primary min-h-11 w-full" onClick={() => setConfirmed(true)}>
          Join {org}
        </button>
        <p className="text-body">Not for you? Close this page; nothing has been created.</p>
      </Message>
    );

  return (
    <AuthShell>
      <div className="flex flex-col gap-4">
        <div className="mb-2">
          <h1 className="connect-title">Join {org}</h1>
          <p className="mt-3 text-reading" style={{ color: "var(--ink-dim)" }}>
            An admin of {org} invited{" "}
            <strong>{inv.email}</strong> to {org}
            {as}. Create your account to join.
          </p>
        </div>
        {provider && (
          <>
            {/* A form post, not a link: the invitation goes to the server in the body, which keeps it in the signed
                sign-in cookie and out of every URL (the provider's included). Only this invitation can be used. */}
            <form method="post" action="/api/auth/oidc/start">
              <input type="hidden" name="next" value="/invite" />
              <input type="hidden" name="invitation" value={token} />
              <button type="submit" className={`btn ${mode === "oidc" ? "btn-primary" : ""} min-h-11 w-full`}>
                <KeyIcon weight="duotone" size={16} aria-hidden />
                Continue with {provider}
              </button>
            </form>
            {mode !== "oidc" && <OrDivider />}
          </>
        )}
        {mode !== "oidc" && (
          <form onSubmit={signUp} className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="email" className={LABEL}>
                Email
              </label>
              <input id="email" type="email" readOnly className="field h-11 px-3 text-reading" value={inv.email} />
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
                  minLength={8}
                  maxLength={72}
                  autoComplete="new-password"
                  className="field h-11 w-full pl-3 pr-16 text-reading"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
                <RevealToggle shown={show} onToggle={() => setShow((s) => !s)} controls="password confirm-password" />
              </div>
            </div>
            <div className="flex flex-col gap-1.5">
              <label htmlFor="confirm-password" className={LABEL}>
                Confirm password
              </label>
              <input
                id="confirm-password"
                type={show ? "text" : "password"}
                required
                minLength={8}
                maxLength={72}
                autoComplete="new-password"
                className="field h-11 px-3 text-reading"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
              />
            </div>
            {error && <ErrorLine error={error} />}
            <button type="submit" disabled={busy} className="btn btn-primary mt-1 min-h-11 w-full">
              {busy ? "Creating account…" : `Create account and join`}
            </button>
          </form>
        )}
        <p className="text-body" style={{ color: "var(--ink-dim)" }}>
          Already have an account?{" "}
          <Link href={signIn} className="link font-medium">
            Sign in
          </Link>{" "}
          and this page will open again.
        </p>
      </div>
    </AuthShell>
  );
}
