"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { ArrowLeft, Loader2, X } from "lucide-react";
import { nextPath } from "@/lib/nextPath";
import { useMe } from "@/lib/queries/auth";
import { useCompleteOnboarding, useSettings, useUpdateSettings } from "@/lib/queries/settings";
import EunomiaMark from "@/components/EunomiaMark";
import ThemeToggle from "@/components/ThemeToggle";
import BlurWords from "@/components/bits/BlurWords";
import { spark } from "@/components/bits/Spark";
import { InputModeTracker } from "@/components/bits/motion";
import ConnectAgents from "@/components/ConnectAgents";
import ResolveMark from "./ResolveMark";

const STEP_LABELS = ["Welcome", "Connect your AI", "OpenAI key"];

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
  // The welcome moment plays once; coming Back shows it at rest.
  const [welcomed, setWelcomed] = useState(false);
  // Skip turns into Continue once a token was minted on the Connect step.
  const [connected, setConnected] = useState(false);

  const { isError, data } = meQuery;
  useEffect(() => {
    if (isError) router.replace("/login");
    else if (data?.onboarded) router.replace(nextPath());
  }, [isError, data, router]);

  // The OpenAI step is pointless if a key is already configured
  // server-wide (env var) or was already set earlier by this user.
  const skipOpenAiStep = Boolean(appSettings?.openai_api_key_set);
  const steps = skipOpenAiStep ? STEP_LABELS.slice(0, 2) : STEP_LABELS;

  /** `from` is the button a pointer click started this from; it sparks on success. Navigation is never delayed. */
  async function finish(from?: Element | null) {
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
      setError(failure(err, "Could not save the key. Check the URL and key, or skip for now."));
      setBusy(false);
    }
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
    <div className="canvas-grid min-h-screen w-full flex flex-col px-6 py-6 sm:px-10">
      <InputModeTracker />
      {/* Progress fills slide in when a pointer moved the step; keyboard steps swap instantly. */}
      <style>{`
        .onb-fill { transform-origin: left center; }
        html[data-input="pointer"] .onb-fill { transition: transform 260ms var(--ease-out); }
      `}</style>
      <header className="flex items-center justify-between">
        <span className="inline-flex items-center gap-2 text-[14px] font-semibold tracking-[-0.01em]">
          <EunomiaMark size={22} />
          Eunomia
        </span>
        <div className="flex items-center gap-1">
          <ThemeToggle />
          {me && (
            <button onClick={() => skip()} disabled={busy} aria-label="Skip setup" title="Skip setup" className="btn btn-ghost btn-icon btn-sm" style={{ width: 26 }}>
              <X size={14} strokeWidth={1.75} />
            </button>
          )}
        </div>
      </header>

      <main className="flex-1 flex items-center justify-center py-10">
        <div className="panel w-full max-w-[520px] p-7">
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
              <ol className="flex gap-2 mb-7" aria-label="Setup progress">
                {steps.map((label, i) => (
                  <li key={label} className="flex-1" aria-current={i === step ? "step" : undefined}>
                    <span className="block h-[3px] rounded-full overflow-hidden" style={{ background: "var(--border-strong)" }}>
                      <span className="onb-fill block h-full" style={{ background: "var(--accent)", transform: i <= step ? "none" : "scaleX(0)" }} />
                    </span>
                    <span className="mt-2 flex items-center gap-1.5 whitespace-nowrap text-[12px]" style={{ color: i === step ? "var(--ink)" : "var(--ink-faint)" }}>
                      <span className="font-mono">{i + 1}</span>
                      {label}
                      {i === 2 && <span className="hidden sm:inline" style={{ color: "var(--ink-faint)" }}>(optional)</span>}
                    </span>
                  </li>
                ))}
              </ol>

              {step === 0 && (
                <div className="flex flex-col gap-4">
                  <ResolveMark size={36} still={welcomed} />
                  {welcomed ? <h1 className="page-title">Welcome to Eunomia</h1> : <BlurWords text="Welcome to Eunomia" as="h1" className="page-title" />}
                  <p className="text-[14px] leading-[1.6]" style={{ color: "var(--ink-dim)" }}>
                    You&apos;re signed in as{" "}
                    <span className="font-medium" style={{ color: "var(--ink)" }}>
                      {me.email}
                    </span>
                    . Next, connect the AI you work with{skipOpenAiStep ? "" : ", then optionally add a model key"}. It can all
                    wait: the dashboard keeps the setup guides, and Connectors adds data sources any time.
                  </p>
                  <button onClick={() => setStep(1)} className="btn btn-primary h-9 self-start mt-2 px-4">
                    Let&apos;s go
                  </button>
                </div>
              )}

              {/* Hidden, not unmounted, so a minted token survives Back. */}
              <div hidden={step !== 1} className="flex flex-col gap-4">
                <h1 className="page-title">Connect your AI</h1>
                <p className="text-[13.5px] leading-[1.6]" style={{ color: "var(--ink-dim)" }}>
                  Your agent recalls and saves memories through Eunomia&apos;s MCP server. It learns when to from the
                  server itself, so there are no skills or hooks to install.
                </p>
                <ConnectAgents onToken={() => setConnected(true)} />
                <div className="flex items-center gap-2 mt-2">
                  <button
                    onClick={() => {
                      setWelcomed(true);
                      setStep(0);
                    }}
                    disabled={busy}
                    className="btn btn-ghost h-9 px-2.5"
                  >
                    <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
                    Back
                  </button>
                  <div className="flex-1" />
                  <button
                    onClick={(e) => (skipOpenAiStep ? skip(e.detail > 0 ? e.currentTarget : null) : setStep(2))}
                    disabled={busy}
                    aria-busy={busy}
                    className={`btn h-9 px-4 ${connected ? "" : "btn-ghost"}`}
                  >
                    {busy && <Loader2 size={14} strokeWidth={1.75} className="animate-spin" aria-hidden />}
                    {connected ? (skipOpenAiStep ? "Finish" : "Continue") : "Skip"}
                  </button>
                </div>
              </div>

              {step === 2 && !skipOpenAiStep && (
                <div className="flex flex-col gap-4">
                  <h1 className="page-title">Add an OpenAI key</h1>
                  <p className="text-[13.5px] leading-[1.6]" style={{ color: "var(--ink-dim)" }}>
                    Connected agents like Claude Code and Codex can recall and write memory with their own model. An
                    OpenAI-compatible endpoint lets Eunomia run embeddings and answers itself, which saves your
                    agents&apos; tokens. You can add it later in Settings.
                  </p>
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="base-url" className="text-[12.5px] font-medium" style={{ color: "var(--ink-dim)" }}>
                      Base URL
                    </label>
                    <input
                      id="base-url"
                      type="url"
                      autoComplete="off"
                      spellCheck={false}
                      className="field h-9 px-3 text-[13px] font-mono"
                      placeholder="https://api.openai.com/v1"
                      value={baseUrlInput}
                      onChange={(e) => setBaseUrlInput(e.target.value)}
                    />
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="api-key" className="text-[12.5px] font-medium" style={{ color: "var(--ink-dim)" }}>
                      API key
                    </label>
                    <input
                      id="api-key"
                      type="password"
                      autoComplete="off"
                      spellCheck={false}
                      className="field h-9 px-3 text-[13px] font-mono"
                      placeholder="sk-…"
                      value={apiKeyInput}
                      onChange={(e) => setApiKeyInput(e.target.value)}
                    />
                  </div>

                  {error && <ErrorLine error={error} />}

                  <div className="flex items-center gap-2 mt-2">
                    <button
                      onClick={() => setStep(1)}
                      disabled={busy} className="btn btn-ghost h-9 px-2.5">
                      <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
                      Back
                    </button>
                    <div className="flex-1" />
                    <button onClick={() => skip()} disabled={busy} className="btn btn-ghost h-9">
                      Skip &amp; finish
                    </button>
                    <button onClick={saveApiKey} disabled={busy} aria-busy={busy} className="btn btn-primary h-9 px-4">
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
