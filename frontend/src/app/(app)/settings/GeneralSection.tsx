"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useState } from "react";
import Link from "next/link";
import { Check, ChevronRight, Plug } from "lucide-react";
import type { SettingsUpdate } from "@/lib/gen";
import { useOpenAiModels, useUpdateSettings } from "@/lib/queries/settings";
import type { AppSettings } from "@/lib/types";
import { ICON, PanelHead } from "./shared";

export function GeneralSection({ settings }: { settings: AppSettings }) {
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
  const [syncError, setSyncError] = useState<Failure | null>(null);

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

  /* Saves at once: it is a single switch, not part of the OpenAI form below. */
  async function setCheckConnectivity(on: boolean) {
    setSyncError(null);
    try {
      await updateSettings.mutateAsync({ sync_check_connectivity: on });
    } catch (e) {
      setSyncError(failure(e, "Could not save the setting. Try again."));
    }
  }

  const row = "grid gap-1.5 md:grid-cols-[180px_minmax(0,1fr)] md:gap-6 px-5 py-4 items-start";

  return (
    <div className="flex flex-col gap-8">
      <div className="flex flex-col gap-3">
        <PanelHead title="Connected sources" />
        <Link href="/connectors" className="ledger group px-4 py-3 flex items-center gap-3 transition-colors hover:bg-[var(--surface-raised)]">
          <Plug {...ICON} style={{ color: "var(--ink-faint)" }} aria-hidden />
          <div className="flex-1 min-w-0">
            <div className="text-[15px] font-semibold">Connected sources</div>
            <div className="label mt-0.5">Up Bank, PocketAI and more: sign-in details and connection status</div>
          </div>
          <span className="text-[12.5px] flex items-center gap-1" style={{ color: "var(--accent-text)" }}>
            Manage
            <ChevronRight {...ICON} aria-hidden />
          </span>
        </Link>
      </div>

      <div className="flex flex-col gap-3">
        <PanelHead title="Syncing" />
        <label htmlFor="check-connectivity" className="ledger px-5 py-4 flex items-start gap-3 cursor-pointer">
          <input
            id="check-connectivity"
            type="checkbox"
            className="mt-0.5"
            style={{ accentColor: "var(--accent)" }}
            checked={settings.sync_check_connectivity}
            disabled={updateSettings.isPending}
            onChange={(e) => setCheckConnectivity(e.target.checked)}
          />
          <span className="flex flex-col gap-0.5">
            <span className="text-[13px] font-medium">Skip syncs while offline</span>
            <span className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
              Checks for an internet connection before each scheduled sync. With no connection the sync is skipped quietly, with no
              error shown, and picks up again by itself when you are back online. Sync now says so instead of failing.
            </span>
          </span>
        </label>
        {syncError && <ErrorLine error={syncError} />}
      </div>

      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          saveSettings();
        }}
      >
        <PanelHead title="OpenAI (optional)">
          Not required: an agent connected over MCP (Claude, Codex and others) can recall and write memory using its own
          model. Adding one saves agent tokens, since Eunomia then does embeddings and answer synthesis itself and powers the
          in-app chat. Any OpenAI-compatible endpoint works, a local model server or proxy included.
        </PanelHead>

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
                <button type="button" onClick={removeKey} disabled={saving} className="btn btn-sm shrink-0">
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
              {saved && <Check {...ICON} />}
              {saving ? "Saving…" : saved ? "Saved" : "Save settings"}
            </button>
          </div>
        </div>
        {error && <ErrorLine error={error} />}
      </form>
    </div>
  );
}
