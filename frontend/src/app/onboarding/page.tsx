"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { ArrowLeft, Loader2, X } from "lucide-react";
import { nextPath } from "@/lib/nextPath";
import { useMe } from "@/lib/queries/auth";
import { useCompleteOnboarding, useSettings, useUpdateSettings } from "@/lib/queries/settings";
import EunomiaMark from "@/components/EunomiaMark";
import { spark } from "@/components/bits/Spark";
import { InputModeTracker } from "@/components/bits/motion";
import ConnectAgents from "@/components/ConnectAgents";
import AppChoices from "@/components/connect/AppChoices";
import InvitationBanner from "@/components/vaults/InvitationBanner";
import type { AppId } from "@/components/connect/apps";

const STEP_LABELS = ["Connect your AI app", "OpenAI key"];

export default function OnboardingPage() {
  const router = useRouter();
  const meQuery = useMe();
  const me = meQuery.data && !meQuery.data.onboarded ? meQuery.data : null;
  const appSettings = useSettings().data ?? null;
  const completeOnboarding = useCompleteOnboarding();
  const updateSettings = useUpdateSettings();
  const [step, setStep] = useState(0);
  // null until typed in: the field then shows the saved base URL.
  const [typedBaseUrl, setBaseUrlInput] = useState<string | null>(null);
  const baseUrlInput = typedBaseUrl ?? appSettings?.openai_base_url ?? "";
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  // A Continue appears once a token was minted on the Connect step (admins only); otherwise "Set up later" is the one exit.
  const [connected, setConnected] = useState(false);

  // A me refresh can finish before the completion request, so only the active handler may navigate.
  const navigating = useRef(false);
  const { isError, data } = meQuery;
  useEffect(() => {
    if (navigating.current) return;
    if (isError) router.replace("/login");
    else if (data?.onboarded) router.replace(nextPath());
  }, [isError, data, router]);

  // The OpenAI step is pointless if a key is already configured server-wide or was set earlier,
  // and members can't set it at all (it is a server setting for admins).
  const isAdmin = Boolean(data?.is_admin);
  const skipOpenAiStep = !isAdmin || Boolean(appSettings?.openai_api_key_set);
  const steps = skipOpenAiStep ? STEP_LABELS.slice(0, 1) : STEP_LABELS;

  /** `from` is the button a pointer click started this from; it sparks on success. Navigation is never delayed. */
  async function finish(from?: Element | null) {
    navigating.current = true;
    await completeOnboarding.mutateAsync();
    spark(from, { count: 8 });
    router.replace(nextPath());
  }

  async function saveApiKey(e: React.MouseEvent<HTMLButtonElement>) {
    const from = e.detail > 0 ? e.currentTarget : null;
    setBusy(true);
    setError(null);
    try {
      const url = baseUrlInput.trim();
      const changes: Record<string, string> = {};
      if (url && url !== appSettings?.openai_base_url) changes.openai_base_url = url;
      if (apiKeyInput) changes.openai_api_key = apiKeyInput;
      if (Object.keys(changes).length) await updateSettings.mutateAsync(changes);
      await finish(from);
    } catch (err) {
      navigating.current = false;
      setError(failure(err, "Could not save the key. Check the URL and key, or skip for now."));
      setBusy(false);
    }
  }

  /** Choosing an app finishes setup and opens that app's guide, the same one Home links to. */
  async function pickApp(id: AppId) {
    navigating.current = true;
    setBusy(true);
    try {
      await completeOnboarding.mutateAsync();
    } catch {
      // The guide is reachable either way; setup can be finished from there.
    }
    router.replace(`/connect/${id}`);
  }

  async function skip(from?: Element | null) {
    setBusy(true);
    try {
      await finish(from);
    } catch {
      // Finishing the wizard shouldn't trap the user even if the server
      // call fails: let them into the app and they can set things up
      // from Settings/Connectors later.
      router.replace(nextPath());
    }
  }

  return (
    <div className="min-h-screen w-full flex flex-col px-6 py-6 sm:px-10" style={{ background: "var(--canvas)" }}>
      <InputModeTracker />
      {/* Progress fills slide in when a pointer moved the step; keyboard steps swap instantly. */}
      <style>{`
        .onb-fill { transform-origin: left center; }
        html[data-input="pointer"] .onb-fill { transition: transform 260ms var(--ease-out); }
      `}</style>
      <header className="flex items-center justify-between">
        <span className="inline-flex items-center gap-2 text-[20px] font-semibold tracking-[-0.03em]">
          <EunomiaMark size={24} />
          eunomia
        </span>
        <div className="flex items-center gap-1">
          {me && (
            <button onClick={() => skip()} disabled={busy} className="btn btn-ghost btn-sm">
              <X size={16} strokeWidth={1.75} aria-hidden />
              Set up later
            </button>
          )}
        </div>
      </header>

      <main className="flex-1 flex items-center justify-center py-10">
        <div className="task-panel w-full max-w-[680px] p-6 sm:p-8">
          {!me ? (
            <div className="flex flex-col gap-4" aria-busy="true" aria-label="Loading">
              <div className="skeleton h-1 w-full" />
              <div className="skeleton h-6 w-2/3 mt-4" />
              <div className="skeleton h-4 w-full" />
              <div className="skeleton h-4 w-4/5" />
              <div className="skeleton h-9 w-28 mt-2" />
            </div>
          ) : (
            <>
              {!isAdmin && <InvitationBanner className="mb-6" />}
              {steps.length > 1 && (
              <ol className="flex gap-2 mb-7" aria-label="Setup progress">
                {steps.map((label, i) => (
                  <li key={label} className="flex-1" aria-current={i === step ? "step" : undefined}>
                    <span className="block h-[4px] overflow-hidden" style={{ background: "var(--border)" }}>
                      <span className="onb-fill block h-full" style={{ background: "var(--accent)", transform: i <= step ? "none" : "scaleX(0)" }} />
                    </span>
                    <span className="mt-2 flex items-center gap-1.5 whitespace-nowrap text-[13px]" style={{ color: i === step ? "var(--ink)" : "var(--ink-dim)", fontWeight: i === step ? 600 : 400 }}>
                      <span className="font-mono">{i + 1}</span>
                      {label}
                      {i === 1 && <span className="hidden sm:inline" style={{ color: "var(--ink-dim)" }}>(optional)</span>}
                    </span>
                  </li>
                ))}
              </ol>
              )}

              {/* Hidden, not unmounted, so a minted token survives Back. */}
              <div hidden={step !== 0} className="flex flex-col gap-5">
                <div className="flex flex-col gap-2">
                  <h1 className="connect-title">Connect your AI app</h1>
                  <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
                    Choose an AI app to connect to your Eunomia memory. You can also do this later from Home.
                  </p>
                </div>
                <AppChoices onPick={pickApp} disabled={busy} />
                {isAdmin && (
                  <details className="border-t pt-3">
                    <summary className="cursor-pointer text-[14px] font-semibold min-h-11 flex items-center">Advanced: use an API token</summary>
                    <div className="pt-3">
                      <ConnectAgents onToken={() => setConnected(true)} />
                    </div>
                  </details>
                )}
                {connected && (
                  <div className="flex items-center justify-end">
                    <button
                      onClick={(e) => (skipOpenAiStep ? skip(e.detail > 0 ? e.currentTarget : null) : setStep(1))}
                      disabled={busy}
                      aria-busy={busy}
                      className="btn btn-primary"
                    >
                      {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                      {skipOpenAiStep ? "Finish" : "Continue"}
                    </button>
                  </div>
                )}
              </div>

              {step === 1 && !skipOpenAiStep && (
                <div className="flex flex-col gap-4">
                  <h1 className="connect-title">Add an OpenAI key</h1>
                  <p className="text-[16px] leading-[1.6]" style={{ color: "var(--ink-dim)" }}>
                    Optional. Your AI app already works without a key. A key adds search by meaning and Chat inside
                    Eunomia. You can add it later in Settings.
                  </p>
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="base-url" className="text-[14px] font-semibold">
                      Base URL
                    </label>
                    <input
                      id="base-url"
                      type="url"
                      autoComplete="off"
                      spellCheck={false}
                      className="field h-11 px-3 text-[14px] font-mono"
                      placeholder="https://api.openai.com/v1"
                      value={baseUrlInput}
                      onChange={(e) => setBaseUrlInput(e.target.value)}
                    />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="api-key" className="text-[14px] font-semibold">
                      API key
                    </label>
                    <input
                      id="api-key"
                      type="password"
                      autoComplete="off"
                      spellCheck={false}
                      className="field h-11 px-3 text-[14px] font-mono"
                      placeholder="sk-…"
                      value={apiKeyInput}
                      onChange={(e) => setApiKeyInput(e.target.value)}
                    />
                  </div>

                  {error && <ErrorLine error={error} />}

                  <div className="flex items-center gap-2 mt-2">
                    <button onClick={() => setStep(0)} disabled={busy} className="btn btn-ghost">
                      <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
                      Back
                    </button>
                    <div className="flex-1" />
                    <button onClick={() => skip()} disabled={busy} className="btn">
                      Skip &amp; finish
                    </button>
                    <button onClick={saveApiKey} disabled={busy} aria-busy={busy} className="btn btn-primary">
                      {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                      {busy ? "Finishing…" : "Finish"}
                    </button>
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      </main>
    </div>
  );
}
