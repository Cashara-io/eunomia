"use client";

import { KeyIcon } from "@phosphor-icons/react/ssr";
import { useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import { bootstrapQuery } from "@/lib/queries/auth";
import { nextPath } from "@/lib/nextPath";
import type { Failure } from "@/components/ErrorLine";

/** How this install lets people in, from the public bootstrap endpoint. `pending` until it answers; a failed
 * request falls back to the password form (the server still refuses what it has turned off). */
export function useSignIn() {
  const q = useQuery(bootstrapQuery());
  const mode = q.data?.auth_mode ?? "password";
  const provider = mode === "password" ? null : (q.data?.oidc_provider ?? "single sign-on");
  return { pending: q.isPending, mode, provider, hasUsers: q.data?.has_users };
}

/** The messages for the `?oidc_error=` codes the callback redirects with. */
function oidcMessage(code: string, provider: string): string | null {
  switch (code) {
    case "state":
      return "That sign-in expired, or was started in another browser. Try again.";
    case "denied":
      return `${provider} did not allow the sign-in.`;
    case "provider":
      return `Could not reach ${provider}. Try again in a moment, or ask the admin to check the provider settings.`;
    case "token":
      return `${provider} sent a sign-in this server could not verify. Try again.`;
    case "no_email":
      return `${provider} did not share an email address, which Eunomia needs to find your account.`;
    case "email_unverified":
      return `An account with this email already exists, and ${provider} has not verified the address. Sign in with your password, or verify the address with ${provider}.`;
    case "invitation":
      return "That invitation can't be used any more. It may have been used or withdrawn. Ask for a new link.";
    case "invitation_expired":
      return "That invitation has expired. Ask the person who invited you to send a new link.";
    case "invitation_email":
      return `The invitation is for a different email address than the one ${provider} signed you in with. Use the invited address.`;
    case "invitation_unverified":
      return `${provider} has not verified your email address, so the invitation can't be used with it. Create an account with a password from the invitation link instead.`;
    case "signup_closed":
      return "This server is not open to new accounts from this email. Ask the admin for access.";
    case "mfa":
      return `${provider} did not confirm a second factor for this sign-in, and this server requires one. Sign in to ${provider} with two-step verification, or ask the admin.`;
    case "unavailable":
      return "Could not finish signing in. Try again.";
    default:
      return null;
  }
}

const noSubscribe = () => () => {};

/** The failure the callback left in the URL, if any. Read after mount (it needs the query string). */
export function useOidcError(provider: string | null): Failure | null {
  const code = useSyncExternalStore(noSubscribe, () => new URLSearchParams(window.location.search).get("oidc_error"), () => null);
  const message = code ? oidcMessage(code, provider ?? "the provider") : null;
  return message ? { message } : null;
}

/** "Continue with {provider}": a full-page navigation to the backend, which redirects to the provider and, after
 * sign-in, back to `next` (so the OAuth consent flow resumes where it left off). */
/** The invitation the invitation page kept in this tab (`/invite` stores it), when sign-in returns there. */
const pendingInvitation = () => (nextPath() === "/invite" ? sessionStorage.getItem("eunomia_invite") : null);

export function SsoButton({ provider, primary }: { provider: string; primary: boolean }) {
  // the query string only exists in the browser; until hydration the link points at the dashboard
  const next = useSyncExternalStore(noSubscribe, nextPath, () => "/");
  const invitation = useSyncExternalStore(noSubscribe, pendingInvitation, () => null);
  // signing in on the way back to an invitation: the same form post the invitation page makes, so this
  // sign-in can use that invitation (and only that one) instead of making an account of its own
  if (invitation)
    return (
      <form method="post" action="/api/auth/oidc/start" className="w-full">
        <input type="hidden" name="next" value="/invite" />
        <input type="hidden" name="invitation" value={invitation} />
        <button type="submit" className={`btn ${primary ? "btn-primary" : ""} min-h-11 w-full`}>
          <KeyIcon weight="duotone" size={16} aria-hidden />
          Continue with {provider}
        </button>
      </form>
    );
  return (
    <a href={`/api/auth/oidc/start?next=${encodeURIComponent(next)}`} className={`btn ${primary ? "btn-primary" : ""} min-h-11 w-full`}>
      <KeyIcon weight="duotone" size={16} aria-hidden />
      Continue with {provider}
    </a>
  );
}

export function OrDivider() {
  return (
    <div className="flex items-center gap-3" role="separator" aria-label="or">
      <span className="h-px flex-1" style={{ background: "var(--border-strong)" }} />
      <span className="label">or</span>
      <span className="h-px flex-1" style={{ background: "var(--border-strong)" }} />
    </div>
  );
}
