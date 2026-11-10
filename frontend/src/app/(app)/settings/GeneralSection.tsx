"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useState } from "react";
import { CheckIcon } from "@phosphor-icons/react/ssr";
import type { SettingsUpdate } from "@/lib/gen";
import { useOpenAiModels, useUpdateSettings } from "@/lib/queries/settings";
import type { AppSettings } from "@/lib/types";
import { ICON, PanelHead, QUIET_DANGER } from "./shared";

/* The AI provider: an admin concern (base URL and key), shown only in the Server group. */
export function ModelSection({ settings }: { settings: AppSettings }) {
  const updateSettings = useUpdateSettings();
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [baseUrlInput, setBaseUrlInput] = useState(settings.openai_base_url);
  const [modelInput, setModelInput] = useState(settings.embedding_model);
  const [chatModelInput, setChatModelInput] = useState(settings.chat_model);
  const modelsQuery = useOpenAiModels(settings.openai_base_url, settings.openai_api_key_set);
  const models = modelsQuery.data?.models ?? [];
  // an endpoint that lists embedding models gets only those; one that lists none keeps the full list
  const embeddingModels = models.some((m) => /embed/i.test(m)) ? models.filter((m) => /embed/i.test(m)) : models;
  const modelsError = modelsQuery.data?.error ?? (modelsQuery.isError ? "Could not reach the models endpoint." : null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<Failure | null>(null);

  const urlInvalid = baseUrlInput.trim() !== "" && !/^https?:\/\/\S+$/.test(baseUrlInput.trim());
  const dirty =
    apiKeyInput !== "" || baseUrlInput.trim() !== settings.openai_base_url || modelInput.trim() !== settings.embedding_model ||
    chatModelInput.trim() !== settings.chat_model;

  async function saveSettings() {
    if (urlInvalid) return;
    const payload: SettingsUpdate = { theme: settings.theme, embedding_model: modelInput.trim(), chat_model: chatModelInput.trim() };
    // The field shows the effective URL (the server's when you have none): only save it once you change it.
    if (baseUrlInput.trim() !== settings.openai_base_url) payload.openai_base_url = baseUrlInput.trim();
    if (apiKeyInput) payload.openai_api_key = apiKeyInput;
    setSaving(true);
    setError(null);
    try {
      await updateSettings.mutateAsync(payload);
      setApiKeyInput("");
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError(failure(e, "Could not save settings. Check the values above and try again."));
    } finally {
      setSaving(false);
    }
  }

  /* An explicit empty key clears the stored one (the server then falls back to its own, if any). */
  async function removeKey() {
    setSaving(true);
    setError(null);
    try {
      await updateSettings.mutateAsync({ openai_api_key: "" });
      setApiKeyInput("");
    } catch (e) {
      setError(failure(e, "Could not remove the key. Try again."));
    } finally {
      setSaving(false);
    }
  }

  const row = "grid gap-1.5 md:grid-cols-[180px_minmax(0,1fr)] md:gap-6 px-5 py-4 items-start";

  return (
    <div className="flex flex-col gap-8">
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          saveSettings();
        }}
      >
        <PanelHead title="AI provider (optional)">
          Add an OpenAI-compatible provider for meaning search and in-app chat.
        </PanelHead>

        <a href="/docs#ai-provider" className="link text-detail self-start">About AI providers</a>

        <div className="ledger hairline-rows">
          <div className={row}>
            <label htmlFor="base-url" className="text-[13px] font-medium md:pt-1.5">
              Base URL
            </label>
            <div className="flex flex-col gap-1">
              <input
                id="base-url"
                className="field h-10 px-3 text-[14px] font-mono"
                value={baseUrlInput}
                onChange={(e) => setBaseUrlInput(e.target.value)}
                placeholder="https://api.openai.com/v1"
                aria-invalid={urlInvalid || undefined}
                style={urlInvalid ? { borderColor: "var(--critical)" } : undefined}
              />
              {urlInvalid && (
                <span className="text-[12px]" style={{ color: "var(--critical)" }}>
                  Start the URL with http:// or https://, for example https://api.openai.com/v1.
                </span>
              )}
            </div>
          </div>

          <div className={row}>
            <label htmlFor="api-key" className="text-[13px] font-medium md:pt-1.5 flex items-center gap-2">
              API key
              {settings.openai_api_key_set && (
                <span className="label flex items-center gap-1.5">
                  <span className="dot" style={{ background: "var(--good)" }} aria-hidden />
                  set
                </span>
              )}
            </label>
            <div className="flex items-center gap-2">
              <input
                id="api-key"
                type="password"
                className="field h-10 px-3 text-[14px] flex-1 min-w-0"
                value={apiKeyInput}
                onChange={(e) => setApiKeyInput(e.target.value)}
                placeholder={settings.openai_api_key_set ? "Leave blank to keep the current key" : "Optional, not every base URL needs one"}
                autoComplete="off"
              />
              {settings.openai_api_key_set && (
                <button type="button" onClick={removeKey} disabled={saving} className={`${QUIET_DANGER} shrink-0`}>
                  Remove key
                </button>
              )}
            </div>
          </div>

          <div className={row}>
            <label htmlFor="model" className="text-[13px] font-medium md:pt-1.5">
              Embedding model
            </label>
            <div className="flex flex-col gap-1">
              {models.length > 0 ? (
                <Select
                  id="model"
                  mono
                  className="h-10 text-[14px] w-full"
                  value={modelInput}
                  onChange={setModelInput}
                  options={[...(!embeddingModels.includes(modelInput) && modelInput ? [modelInput] : []), ...embeddingModels].map((m) => ({ value: m, label: m }))}
                />
              ) : (
                <input
                  id="model"
                  className="field h-10 px-3 text-[14px] font-mono"
                  value={modelInput}
                  onChange={(e) => setModelInput(e.target.value)}
                  placeholder="text-embedding-3-small"
                />
              )}
              {modelsError && (
                <span className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
                  Couldn&apos;t list models from this endpoint ({modelsError}). Type a model id directly.
                </span>
              )}
            </div>
          </div>

          <div className={row}>
            <label htmlFor="chat-model" className="text-[13px] font-medium md:pt-1.5">
              Chat model
            </label>
            <div className="flex flex-col gap-1">
              {models.length > 0 ? (
                <Select
                  id="chat-model"
                  mono
                  className="h-10 text-[14px] w-full"
                  value={chatModelInput}
                  onChange={setChatModelInput}
                  options={[
                    { value: "", label: "Automatic" },
                    ...(!models.includes(chatModelInput) && chatModelInput ? [chatModelInput] : []).map((m) => ({ value: m, label: m })),
                    ...models.map((m) => ({ value: m, label: m })),
                  ]}
                />
              ) : (
                <input
                  id="chat-model"
                  className="field h-10 px-3 text-[14px] font-mono"
                  value={chatModelInput}
                  onChange={(e) => setChatModelInput(e.target.value)}
                  placeholder="automatic"
                />
              )}
              <span className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
                Finds people, topics and how they connect in your synced data, and powers chat. Blank picks one: gpt-4o-mini on OpenAI,
                otherwise the first chat model this endpoint serves.
              </span>
            </div>
          </div>

          <div className="px-5 py-3 flex items-center justify-end gap-3" style={{ background: "var(--surface-raised)", borderRadius: "0 0 var(--radius-card) var(--radius-card)" }}>
            <span className="label mr-auto" aria-live="polite">
              {saved ? "Saved." : dirty ? "Unsaved changes" : ""}
            </span>
            <button type="submit" disabled={saving || urlInvalid} className="btn btn-primary">
              {saved && <CheckIcon {...ICON} />}
              {saving ? "Saving…" : saved ? "Saved" : "Save settings"}
            </button>
          </div>
        </div>
        {error && <ErrorLine error={error} />}
      </form>
    </div>
  );
}
