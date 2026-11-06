"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useEffect, useState } from "react";
import { Loader2, ShieldAlert } from "lucide-react";
import { getMe } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import { useConsent, useDecideConsent } from "@/lib/queries/oauth";
import EunomiaMark from "@/components/EunomiaMark";
import { InputModeTracker } from "@/components/bits/motion";

// Each permission in plain words: what the app can do, and what that means for you.
const SCOPE_WORDS: Record<string, { title: string; detail: string }> = {
  "memory:read": { title: "Read your memories", detail: "Look up what you and your other AI apps have saved, in the memory spaces you can see." },
  "memory:write": { title: "Save memories", detail: "Add new memories and change or delete ones it saved, as you." },
  "vaults:admin": { title: "Manage your memory spaces", detail: "Create, rename, share and delete memory spaces." },
  connectors: { title: "Manage your connected sources", detail: "See and sync sources like GitHub or Gmail that you connected to Eunomia." },
};

/** The OAuth consent screen. /oauth/authorize validates the request and sends the browser here
 * with the same query string; Allow or Deny posts the decision and follows the redirect the
 * server returns (the client's callback with a one-time code, or the denial). */
export default function ConsentPage() {
  const [error, setError] = useState<Failure | null>(null);
  const [busy, setBusy] = useState<"allow" | "deny" | null>(null);
  const [logoFailed, setLogoFailed] = useState(false);
  const decideConsent = useDecideConsent();
  // Read on the client only; until then the query stays disabled and the skeleton shows.
  const params = typeof window === "undefined" ? null : Object.fromEntries(new URLSearchParams(window.location.search));
  const consent = useConsent(params);
  const info = consent.data ?? null;
  const loadError = consent.error;

  useEffect(() => {
    if (!loadError) return;
    (async () => {
      // No session: sign in first, then come straight back here.
      const signedIn = await call(getMe()).then(() => true, () => false);
      if (!signedIn) {
        const back = encodeURIComponent(`/consent?${window.location.search.slice(1)}`);
        window.location.replace(`/login?next=${back}`);
        return;
      }
      setError(failure(loadError, "This connection request is not valid. Start it again from the app."));
    })();
  }, [loadError]);

  async function decide(approve: boolean) {
    setBusy(approve ? "allow" : "deny");
    setError(null);
    try {
      const { redirect_to } = await decideConsent.mutateAsync({ params: params ?? {}, approve });
      window.location.assign(redirect_to);
    } catch (e) {
      setError(failure(e, "Could not complete the request. Start it again from the app."));
      setBusy(null);
    }
  }

  return (
    <div className="min-h-screen w-full flex flex-col px-6 py-6 sm:px-10" style={{ background: "var(--canvas)" }}>
      <InputModeTracker />
      <header className="flex items-center justify-between">
        <span className="inline-flex items-center gap-2 text-[20px] font-semibold tracking-[-0.03em]">
          <EunomiaMark size={24} />
          eunomia
        </span>
      </header>

      <main className="flex-1 flex items-center justify-center py-10">
        <div className="task-panel w-full max-w-[560px] p-6 sm:p-8">
          {!info ? (
            error ? (
              <div className="flex flex-col gap-3">
                <h1 className="connect-title">Can&apos;t connect this app</h1>
                <ErrorLine error={error} />
              </div>
            ) : (
              <div className="flex flex-col gap-4" aria-busy="true" aria-label="Loading">
                <div className="skeleton h-6 w-2/3" />
                <div className="skeleton h-4 w-full" />
                <div className="skeleton h-20 w-full" />
              </div>
            )
          ) : (
            <div className="flex flex-col gap-5">
              <div className="flex items-center gap-3">
                {info.client.logo_uri && !logoFailed ? (
                  // eslint-disable-next-line @next/next/no-img-element
                  <img
                    src={info.client.logo_uri}
                    alt=""
                    width={36}
                    height={36}
                    referrerPolicy="no-referrer"
                    onError={() => setLogoFailed(true)}
                    className="rounded-[8px] shrink-0"
                    style={{ border: "var(--hair) solid var(--border)", background: "var(--surface-raised)" }}
                  />
                ) : null}
                <h1 className="connect-title">Allow {info.client.name} to use Eunomia?</h1>
              </div>

              <p className="text-[16px] leading-[1.55]" style={{ color: "var(--ink)" }}>
                <span className="font-medium" style={{ color: "var(--ink)" }}>
                  {info.client.name}
                </span>{" "}
                is asking to use your Eunomia memory as{" "}
                <span className="font-medium" style={{ color: "var(--ink)" }}>
                  {info.user_email}
                </span>
                . If you choose Allow, it will be able to:
              </p>

              <ul className="ledger hairline-rows">
                {info.scopes.map((s) => {
                  const words = SCOPE_WORDS[s.scope];
                  return (
                    <li key={s.scope} className="px-4 py-3 flex flex-col gap-0.5">
                      <span className="text-[15px] font-semibold">{words?.title ?? s.description}</span>
                      {words && (
                        <span className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                          {words.detail}
                        </span>
                      )}
                    </li>
                  );
                })}
              </ul>

              <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                It cannot see your password. After you decide, you go back to{" "}
                <code className="font-mono" style={{ color: "var(--ink-dim)" }}>
                  {info.redirect_host}
                </code>
                .
              </p>

              {info.loopback && (
                <div
                  className="flex items-start gap-2 rounded-[10px] px-3 py-2.5 text-[14px] leading-[1.45]"
                  style={{ background: "var(--critical-soft)", color: "var(--critical)" }}
                >
                  <ShieldAlert size={18} strokeWidth={1.75} className="mt-[2px] shrink-0" aria-hidden />
                  <span>This app is on your own computer. Only continue if you just started this connection from it.</span>
                </div>
              )}

              {error && <ErrorLine error={error} />}

              <div className="flex flex-wrap gap-2">
                <button onClick={() => decide(true)} disabled={busy !== null} aria-busy={busy === "allow"} className="btn btn-primary min-h-11 px-5">
                  {busy === "allow" && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                  Allow
                </button>
                <button onClick={() => decide(false)} disabled={busy !== null} aria-busy={busy === "deny"} className="btn min-h-11 px-5">
                  Deny
                </button>
              </div>
              <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                Deny sends you back without connecting. You can disconnect an allowed app any time in Settings, under Connected apps.
              </p>
            </div>
          )}
        </div>
      </main>
    </div>
  );
}
