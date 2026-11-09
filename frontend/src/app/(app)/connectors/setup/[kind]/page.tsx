"use client";

import { ArrowRightIcon, CaretRightIcon, EyeIcon, EyeSlashIcon } from "@phosphor-icons/react/ssr";
import Select from "@/components/Select";
import { use, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import Link from "next/link";
import { apiOrigin } from "@/lib/api";
import type { ConnectorKind, ConnectorUpdate } from "@/lib/types";
import { useMe } from "@/lib/queries/auth";
import { useConnectors, useCreateConnector, useDeleteConnector, useTestConnector, useUpdateConnector } from "@/lib/queries/connectors";
import { useSettings, useUpdateSettings } from "@/lib/queries/settings";
import { useSyncSource } from "@/lib/queries/sources";
import { CONNECTOR_META, CONNECTOR_ORDER, ConnectorTile, connectorStatus, type FieldDef } from "@/lib/connectorMeta";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import CopyButton from "@/components/bits/CopyButton";
import SyncMark from "@/components/bits/SyncMark";
import Tooltip from "@/components/bits/Tooltip";
import PageHeader from "@/components/ui/PageHeader";
import StatusLabel, { toneFromVar } from "@/components/ui/StatusLabel";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import RoutingSection from "./RoutingSection";
import PocketAudioSetting from "@/components/pocket/PocketAudioSetting";

// Presets for the sync-interval select, in seconds.
const SYNC_INTERVAL_PRESETS = [
  { value: 300, label: "5 minutes" },
  { value: 900, label: "15 minutes" },
  { value: 1800, label: "30 minutes" },
  { value: 3600, label: "1 hour" },
  { value: 21600, label: "6 hours" },
  { value: 86400, label: "24 hours" },
];

// Fallback interval a source runs at when the user hasn't set an override,
// per the backend scheduler (heypocket defaults to 24h there, ed to 30 min;
// every other source falls back to the scheduler's generic 900s/15min default).
const SOURCE_DEFAULT_INTERVAL: Record<string, number> = {
  up_bank: 900,
  heypocket: 86400,
  ed: 1800,
};

function isConnectorKind(kind: string): kind is ConnectorKind {
  return (CONNECTOR_ORDER as string[]).includes(kind);
}

/** Webhook signing secrets are optional: polling works without them. */
const isOptional = (f: FieldDef) => f.key.includes("webhook");

/** Names the problem and the fix for one field value, or null when it is fine. */
function validate(f: FieldDef, value: string, required: boolean, connector: string): string | null {
  const v = value.trim();
  if (!v) return required && f.secret && !isOptional(f) ? `Enter your ${f.label.toLowerCase()} to connect ${connector}.` : null;
  if (f.key === "base_url" && !/^https?:\/\/[^\s/]+/i.test(v)) {
    return `${f.label} must be a full address starting with http:// or https://, for example ${f.placeholder}.`;
  }
  if (f.key === "channel_id" && !/^\d{5,}$/.test(v)) {
    return "Channel ID is a long number. In Discord, right-click the channel and choose Copy Channel ID.";
  }
  return null;
}

function Breadcrumb({ label }: { label: string }) {
  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-1.5 text-[13px]" style={{ color: "var(--ink-dim)" }}>
      <Link href="/connectors" className="link">
        Connected sources
      </Link>
      <CaretRightIcon weight="duotone" size={12} aria-hidden />
      <span aria-current="page">{label}</span>
    </nav>
  );
}

export default function ConnectorSetupPage({ params }: { params: Promise<{ kind: string }> }) {
  const { kind } = use(params);

  if (!isConnectorKind(kind)) {
    return (
      <div className="max-w-2xl flex flex-col gap-6">
        <Breadcrumb label={kind} />
        <PageHeader title="Unknown connector" />
        <div className="ledger px-5 py-4 flex items-center justify-between gap-4 flex-wrap text-[14px]">
          <span style={{ color: "var(--ink-dim)" }}>There is no source called &ldquo;{kind}&rdquo;.</span>
          <Link href="/connectors" className="btn">
            Browse connected sources
          </Link>
        </div>
      </div>
    );
  }

  return <ConnectorSetup kind={kind} />;
}

function SecretToggle({ shown, onToggle, label }: { shown: boolean; onToggle: () => void; label: string }) {
  const name = shown ? `Hide ${label}` : `Show ${label}`;
  return (
    <Tooltip label={name}>
      <button type="button" onClick={onToggle} aria-label={name} aria-pressed={shown} className="btn btn-ghost btn-sm btn-icon -mr-2">
        {shown ? <EyeSlashIcon weight="duotone" size={18} aria-hidden /> : <EyeIcon weight="duotone" size={18} aria-hidden />}
      </button>
    </Tooltip>
  );
}

function ConnectorSetup({ kind }: { kind: ConnectorKind }) {
  const meta = CONNECTOR_META[kind];
  const router = useRouter();
  const search = useSearchParams();
  // ?connector=<id> picks one connection of the kind, ?new=1 sets up another one, neither is the kind's first.
  const connectorId = search.get("connector");
  const isNew = !connectorId && search.get("new") === "1";
  const connectorsQuery = useConnectors();
  // null while loading, undefined when the backend lists no such connection (and while adding a new one).
  const connector = connectorsQuery.data
    ? isNew
      ? undefined
      : connectorsQuery.data.find((c) => (connectorId ? c.id === connectorId : c.kind === kind && !c.source_key.includes("~")))
    : null;
  // An added connection (not the kind's first) can be removed outright.
  const added = Boolean(connector?.source_key.includes("~"));
  const ref = connector?.id ?? kind;
  const createConnector = useCreateConnector();
  const deleteConnector = useDeleteConnector();
  const [name, setName] = useState<string | null>(null);
  const [nameError, setNameError] = useState("");
  const loadError = connectorsQuery.isError ? failure(connectorsQuery.error, "Could not load this connector.", " Reload the page once Eunomia is reachable.") : null;
  const updateConnector = useUpdateConnector();
  const testConnector = useTestConnector();
  const updateSettings = useUpdateSettings();
  const syncSource = useSyncSource();
  const [syncing, setSyncing] = useState(false);
  const [syncMessage, setSyncMessage] = useState<{ state: "done" | "partial" | "failed"; text: string } | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [shown, setShown] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<Failure | null>(null);
  const [savedFlash, setSavedFlash] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; error?: Failure } | null>(null);
  const appSettings = useSettings(Boolean(meta.sourceKey)).data;
  const [intervalState, setIntervalState] = useState<"idle" | "saved" | "error">("idle");
  const me = useMe(Boolean(meta.webhooks)).data;
  // The URL is editable (swap in a tunnel host before copying); null until then shows the derived one.
  const [editedWebhookUrl, setWebhookUrl] = useState<string | null>(null);
  const webhookUrl = editedWebhookUrl ?? (me ? `${apiOrigin()}/api/sources/${kind}/webhook/${me.id}` : null);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  // The key instructions start open until the source is connected; then they fold away.
  const [helpChoice, setHelpChoice] = useState<boolean | null>(null);
  const [disconnecting, setDisconnecting] = useState(false);

  const status = connectorStatus(connector ?? undefined);
  const isDemo = status === "demo";
  const connected = status === "connected";

  function setField(key: string, value: string) {
    setValues((s) => ({ ...s, [key]: value }));
    if (errors[key]) setErrors((s) => ({ ...s, [key]: "" }));
  }

  async function test() {
    setTesting(true);
    setTestResult(null);
    try {
      const r = await testConnector.mutateAsync(ref);
      setTestResult({ ok: r.ok, error: r.ok ? undefined : { message: r.error || "The provider gave no reason." } });
    } catch (e) {
      setTestResult({ ok: false, error: failure(e, "The test could not run.") });
    } finally {
      setTesting(false);
    }
  }

  async function save(e: React.FormEvent) {
    e.preventDefault();
    const nextErrors: Record<string, string> = {};
    for (const f of meta.fields) {
      const msg = validate(f, values[f.key] ?? "", !connected, meta.label);
      if (msg) nextErrors[f.key] = msg;
    }
    setErrors(nextErrors);
    const label = (name ?? connector?.label ?? "").trim();
    const missingName = isNew && !label ? "Name this connection, for example the account's email address, so you can tell it apart." : "";
    setNameError(missingName);
    if (missingName) {
      document.getElementById("field-connection-name")?.focus();
      return;
    }
    const firstBad = meta.fields.find((f) => nextErrors[f.key]);
    if (firstBad) {
      document.getElementById(`field-${firstBad.key}`)?.focus();
      return;
    }

    const credentials: Record<string, string> = {};
    const config: Record<string, string> = {};
    for (const f of meta.fields) {
      const v = values[f.key]?.trim();
      if (!v) continue;
      if (f.secret || f.key === "client_id") credentials[f.key] = v;
      else config[f.key] = v;
    }
    const body: ConnectorUpdate = { enabled: true };
    if (name !== null && label !== (connector?.label ?? "")) body.label = label;
    if (Object.keys(credentials).length) body.credentials = credentials;
    if (Object.keys(config).length) body.config = config;

    setSaving(true);
    setSaveError(null);
    let saved;
    try {
      const target = isNew ? (await createConnector.mutateAsync({ kind, label })).id : ref;
      saved = await updateConnector.mutateAsync({ connector: target, body });
    } catch (err) {
      setSaveError(failure(err, "Could not save.", " Check the values and try again."));
      return;
    } finally {
      setSaving(false);
    }
    if (isNew) router.replace(`/connectors/setup/${kind}?connector=${encodeURIComponent(saved.id)}`);
    setName(null);
    setValues({});
    setShown({});
    setSavedFlash(true);
    setTimeout(() => setSavedFlash(false), 1400);
    // Saving is the moment people want to know it works: check it for them.
    if (!isNew) test();
  }

  async function syncNow() {
    if (!meta.sourceKey || !connector) return;
    setSyncing(true);
    setSyncMessage(null);
    try {
      const r = await syncSource.mutateAsync(connector.source_key);
      setSyncMessage(
        r.error
          ? { state: "failed", text: `Sync failed: ${r.error}` }
          : r.status === "already_running"
            ? { state: "done", text: "An update is already running. Its items appear when it finishes" }
            : r.status === "offline"
              ? { state: "failed", text: "You're offline, so nothing was synced. Try again when you have a connection" }
              : r.status === "queued"
                ? { state: "done", text: "Update queued. It starts as soon as Eunomia is free; items appear when it finishes" }
                : r.status === "partial"
                  ? { state: "partial", text: `${r.message ?? "Import in progress"}. Eunomia will continue automatically` }
                  : { state: "done", text: `Synced: ${r.written ?? 0} new or changed, ${r.skipped ?? 0} unchanged` },
      );
    } catch (e) {
      setSyncMessage({ state: "failed", text: `Sync failed: ${failure(e, "unknown error").message}` });
    } finally {
      setSyncing(false);
    }
  }

  async function saveInterval(seconds: number) {
    if (!meta.sourceKey) return;
    try {
      await updateSettings.mutateAsync({
        sync_intervals: { ...(appSettings?.sync_intervals ?? {}), [meta.sourceKey]: seconds },
      });
      setIntervalState("saved");
      setTimeout(() => setIntervalState("idle"), 1400);
    } catch {
      setIntervalState("error");
    }
  }

  /** Stops importing; credentials and everything already imported stay, so connecting again is one click. */
  async function disconnect() {
    setDisconnecting(true);
    setSaveError(null);
    try {
      await updateConnector.mutateAsync({ connector: ref, body: { enabled: false } });
      setConfirmDisconnect(false);
      setTestResult(null);
    } catch (err) {
      setSaveError(failure(err, "Could not disconnect.", " Try again."));
    } finally {
      setDisconnecting(false);
    }
  }

  /** An added connection only: deletes it and everything imported through it. */
  async function remove() {
    setDisconnecting(true);
    setSaveError(null);
    try {
      await deleteConnector.mutateAsync(ref);
      router.push("/connectors");
    } catch (err) {
      setSaveError(failure(err, "Could not remove this connection.", " Try again."));
      setDisconnecting(false);
    }
  }

  /** Switches a disconnected connection back on with the key it already has. */
  async function reconnect() {
    setDisconnecting(true);
    setSaveError(null);
    try {
      await updateConnector.mutateAsync({ connector: ref, body: { enabled: true } });
    } catch (err) {
      setSaveError(failure(err, "Could not connect again.", " Try again."));
    } finally {
      setDisconnecting(false);
    }
  }

  // Credentials the provider just turned down are not "Connected", whatever was saved.
  const rejected = connected && testResult !== null && !testResult.ok;
  const statusText =
    connector === null
      ? "Checking…"
      : isNew
        ? "New connection"
        : isDemo
          ? "Example data"
          : rejected
            ? "Sign-in details rejected"
            : testResult?.ok
              ? "Connected · connection works"
              : connected
                ? "Connected"
                : "Not connected";
  const statusTone = isDemo ? "var(--warning)" : rejected ? "var(--critical)" : connected ? "var(--good)" : "var(--ink-faint)";
  const sourceKey = meta.sourceKey;
  const override = sourceKey ? appSettings?.sync_intervals?.[sourceKey] : undefined;
  const defaultInterval = sourceKey ? SOURCE_DEFAULT_INTERVAL[sourceKey] ?? 900 : 900;

  const showHelp = helpChoice ?? !(connected || isDemo);
  const stepTitle = "text-[18px] font-semibold tracking-[-0.01em]";
  const fieldBox = "field flex items-center gap-2 h-10 px-3";

  return (
    <div className="max-w-5xl flex flex-col gap-8">
      <div className="flex flex-col gap-3">
        <Breadcrumb label={connector?.label ? `${meta.label} (${connector.label})` : meta.label} />
        <PageHeader
          mark={<ConnectorTile kind={kind} size={32} />}
          title={isNew ? `Add another ${meta.label} connection` : connector?.label ? `${meta.label}: ${connector.label}` : meta.label}
          intro={meta.description}
          actions={
            (connected || isDemo) && sourceKey && connector ? (
              <Link href={`/connectors/${connector.source_key}`} className="btn">
                View synced data <ArrowRightIcon weight="duotone" size={18} aria-hidden />
              </Link>
            ) : undefined
          }
        >
          {connector === null ? (
            <span className="skeleton h-4 w-24" aria-label="Checking status" />
          ) : (
            <StatusLabel tone={toneFromVar(statusTone)}>{statusText}</StatusLabel>
          )}
        </PageHeader>
      </div>

      {loadError && <ErrorLine error={loadError} />}

      {isDemo && (
        <p className="soft-panel text-[14px] px-4 py-3" style={{ color: "var(--ink-dim)" }}>
          This source is showing example data. Save your own key below to switch over.
        </p>
      )}

      <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_minmax(0,340px)] items-start">
        <div className="flex flex-col gap-8 min-w-0">
          <form onSubmit={save} noValidate className="flex flex-col gap-8">
            <section className="flex flex-col gap-3" aria-labelledby="step-key">
              <h2 id="step-key" className={stepTitle}>
                1. Get your connection key
              </h2>
              <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                {meta.label} gives you a key that lets Eunomia read your items. It is stored encrypted and only sent to {meta.label}.
              </p>
              {meta.help && (
                <>
                  <button type="button" className="btn self-start" aria-expanded={showHelp} aria-controls="key-help" onClick={() => setHelpChoice(!showHelp)}>
                    {showHelp ? `Hide ${meta.label} instructions` : `Show ${meta.label} instructions`}
                  </button>
                  <div id="key-help" hidden={!showHelp} className="soft-panel p-4 text-[14px] leading-relaxed break-words">
                    {meta.help}
                  </div>
                </>
              )}
            </section>

            <section className="flex flex-col gap-5 pt-6 border-t" aria-labelledby="step-save">
              <h2 id="step-save" className={stepTitle}>
                2. Paste your details and save
              </h2>

              <div className="flex flex-col gap-1.5">
                <label htmlFor="field-connection-name" className="text-[14px] font-medium">
                  Connection name {!isNew && <span style={{ color: "var(--ink-dim)" }}>(optional)</span>}
                </label>
                <div className={fieldBox} style={nameError ? { borderColor: "var(--critical)" } : undefined}>
                  <input
                    id="field-connection-name"
                    type="text"
                    autoComplete="off"
                    maxLength={100}
                    className="flex-1 min-w-0 bg-transparent outline-none text-[14px]"
                    placeholder="For example work@example.com or Kitchen Pocket"
                    value={name ?? connector?.label ?? ""}
                    onChange={(e) => {
                      setName(e.target.value);
                      if (nameError) setNameError("");
                    }}
                    aria-invalid={nameError ? true : undefined}
                    aria-describedby={nameError ? "err-connection-name" : "help-connection-name"}
                  />
                </div>
                {nameError ? (
                  <p id="err-connection-name" className="text-[13px]" style={{ color: "var(--critical)" }}>
                    {nameError}
                  </p>
                ) : (
                  <p id="help-connection-name" className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                    Tells this {meta.label} connection apart from others of the same kind.
                  </p>
                )}
              </div>

              {meta.fields.map((f) => {
                const err = errors[f.key];
                const reveal = shown[f.key];
                const describedBy = err ? `err-${f.key}` : undefined;
                return (
                  <div key={f.key} className="flex flex-col gap-1.5">
                    <label htmlFor={`field-${f.key}`} className="text-[14px] font-medium">
                      {f.label} {isOptional(f) && <span style={{ color: "var(--ink-dim)" }}>(optional)</span>}
                    </label>
                    <div className={fieldBox} style={err ? { borderColor: "var(--critical)" } : undefined}>
                      <input
                        id={`field-${f.key}`}
                        type={f.secret && !reveal ? "password" : "text"}
                        autoComplete="off"
                        spellCheck={false}
                        className="flex-1 min-w-0 bg-transparent outline-none text-[14px] font-mono"
                        placeholder={connected && f.secret ? "Saved. Paste a new value to replace it" : f.placeholder}
                        value={values[f.key] || ""}
                        onChange={(e) => setField(f.key, e.target.value)}
                        aria-invalid={err ? true : undefined}
                        aria-describedby={describedBy}
                      />
                      {f.secret && <SecretToggle shown={!!reveal} label={f.label} onToggle={() => setShown((s) => ({ ...s, [f.key]: !s[f.key] }))} />}
                    </div>
                    {err && (
                      <p id={describedBy} className="text-[13px]" style={{ color: "var(--critical)" }}>
                        {err}
                      </p>
                    )}
                  </div>
                );
              })}

              {meta.webhooks && webhookUrl !== null && (
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="field-webhook-url" className="text-[14px] font-medium">
                    Webhook URL <span style={{ color: "var(--ink-dim)" }}>(optional, for instant updates)</span>
                  </label>
                  <div className="flex items-center gap-2">
                    <input
                      id="field-webhook-url"
                      type="text"
                      spellCheck={false}
                      className="field flex-1 min-w-0 h-10 px-3 text-[13px] font-mono"
                      value={webhookUrl}
                      onChange={(e) => setWebhookUrl(e.target.value)}
                      aria-describedby="webhook-help"
                    />
                    <CopyButton value={() => webhookUrl} size="sm" className="h-10 w-[84px]" />
                  </div>
                  <p id="webhook-help" className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                    Register this with {meta.label} so updates arrive straight away instead of on the next scheduled update. If {meta.label} needs a
                    public address, replace <span className="font-mono">{new URL(apiOrigin()).host}</span> with your tunnel host before copying.
                  </p>
                </div>
              )}

              {saveError && <ErrorLine error={saveError} />}

              <div>
                <button type="submit" disabled={saving} className="btn btn-primary min-w-[96px]">
                  {savedFlash ? "Saved" : saving ? "Saving…" : "Save"}
                </button>
              </div>
            </section>
          </form>

          <section className="flex flex-col gap-4 pt-6 border-t" aria-labelledby="step-check">
            <h2 id="step-check" className={stepTitle}>
              3. Check and import your items
            </h2>
            <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
              After saving, choose Test connection to check that {meta.label} accepts your details.
              {sourceKey ? " Then choose Sync now to bring in your items. You can leave this page while Eunomia imports them." : ""}
            </p>
            <div className="flex items-center gap-3 flex-wrap">
              <button type="button" onClick={test} disabled={testing || !(connected || isDemo)} className="btn" title={connected || isDemo ? undefined : "Save credentials first"}>
                <SyncMark status={testing ? "running" : testResult ? (testResult.ok ? "done" : "failed") : "idle"} size={18} />
                {testing ? "Testing…" : "Test connection"}
              </button>
              <span className="text-[14px] flex items-center gap-1.5 min-w-0" aria-live="polite">
                {testResult?.ok && <StatusLabel tone="good">Connection works</StatusLabel>}
              </span>
            </div>
            {!(connected || isDemo) && (
              <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                Save your details first; then you can check the connection.
              </p>
            )}
            {testResult && !testResult.ok && (
              <ErrorLine error={testResult.error}>
                <span className="font-medium">{meta.label} did not accept the sign-in details.</span> {testResult.error?.message}{" "}
                <span style={{ color: "var(--ink-dim)" }}>Check the key is current and has the access described in step 1, then save it again.</span>
              </ErrorLine>
            )}

            {sourceKey && connected && (
              <div className="flex flex-col gap-2">
                <div className="flex items-center gap-3 flex-wrap">
                  <button type="button" onClick={syncNow} disabled={syncing} className="btn btn-primary">
                    <SyncMark status={syncing ? "running" : syncMessage ? (syncMessage.state === "partial" ? "idle" : syncMessage.state) : "idle"} size={18} />
                    {syncing ? "Syncing…" : "Sync now"}
                  </button>
                  {syncing && (
                    <span className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                      Importing from {meta.label}…
                    </span>
                  )}
                </div>
                {syncMessage && (
                  <span role="status" className="text-[14px] break-words" style={{ color: syncMessage.state === "partial" ? "var(--warning)" : syncMessage.state === "done" ? "var(--good)" : "var(--critical)" }}>
                    {syncMessage.text}
                  </span>
                )}
                {syncMessage?.state === "failed" && (
                  <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                    If this mentions sign-in or permission (HTTP 401 or 403), update your key in step 2. Otherwise try Sync now again; items already
                    imported are kept.
                  </p>
                )}
              </div>
            )}
          </section>

          {sourceKey && (
            <section className="flex flex-col gap-3 pt-6 border-t" aria-labelledby="sync-heading">
              <h2 id="sync-heading" className="section-title">
                Automatic updates
              </h2>
              <div className="flex items-end justify-between gap-4 flex-wrap">
                <label htmlFor="sync-interval" className="flex flex-col gap-0.5 text-[14px] font-medium">
                  Sync interval
                  <span className="text-[13px] font-normal" style={{ color: "var(--ink-dim)" }}>
                    How often Eunomia checks {meta.label} for new items.
                  </span>
                </label>
                <div className="flex items-center gap-2">
                  <span className="text-[13px] w-16 text-right" aria-live="polite" style={{ color: intervalState === "error" ? "var(--critical)" : "var(--good)" }}>
                    {intervalState === "saved" ? "Saved" : intervalState === "error" ? "Not saved" : ""}
                  </span>
                  <Select
                    id="sync-interval"
                    className="h-10 text-[14px] w-48"
                    value={String(override ?? defaultInterval)}
                    onChange={(v) => saveInterval(Number(v))}
                    options={SYNC_INTERVAL_PRESETS.map((p) => ({
                      value: String(p.value),
                      label: p.label,
                      hint: override === undefined && p.value === defaultInterval ? "default" : undefined,
                    }))}
                  />
                </div>
              </div>
            </section>
          )}

          {kind === "pocketai" && connector && <PocketAudioSetting connector={connector} />}

          {connector && !connected && !isDemo && connector.credentials_set && !connector.enabled && (
            <section className="flex flex-col gap-3 pt-6 border-t" aria-labelledby="paused-heading">
              <h2 id="paused-heading" className="section-title">
                Disconnected
              </h2>
              <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                Eunomia isn&apos;t importing from {meta.label} right now. Everything it already brought in is still here.
              </p>
              <button type="button" className="btn btn-primary self-start" disabled={disconnecting} onClick={reconnect}>
                Connect again
              </button>
            </section>
          )}

          {(connected || added) && (
            <section className="flex flex-col gap-3 pt-6 border-t" aria-labelledby="danger-heading">
              <h2 id="danger-heading" className="section-title">
                Stop using this connection
              </h2>
              <div className="flex flex-col gap-4">
                {connected && (
                  <div className="flex flex-col gap-2">
                    <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                      <strong style={{ color: "var(--ink)" }}>Disconnect</strong> stops importing from {meta.label}. Everything already imported stays,
                      and you can connect again any time.
                    </p>
                    <button type="button" className="btn self-start" disabled={disconnecting} onClick={() => setConfirmDisconnect(true)}>
                      Disconnect
                    </button>
                  </div>
                )}
                {added && (
                  <div className="flex flex-col gap-2">
                    <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
                      <strong style={{ color: "var(--ink)" }}>Remove</strong> deletes this connection and everything imported through it. Other{" "}
                      {meta.label} connections stay.
                    </p>
                    <button type="button" className="btn btn-danger self-start" disabled={disconnecting} onClick={() => setConfirmRemove(true)}>
                      Remove and delete its data
                    </button>
                  </div>
                )}
              </div>
              <ConfirmDialog
                open={confirmDisconnect}
                title={`Disconnect ${connector?.label ? `${meta.label} (${connector.label})` : meta.label}?`}
                confirmLabel="Disconnect"
                busy={disconnecting}
                error={saveError?.message}
                onConfirm={disconnect}
                onCancel={() => setConfirmDisconnect(false)}
              >
                <p>Eunomia stops importing from {meta.label}. Items already imported stay in your memory, and you can connect again at any time.</p>
              </ConfirmDialog>
              <ConfirmDialog
                open={confirmRemove}
                danger
                title={`Remove ${connector?.label ? `${meta.label} (${connector.label})` : `this ${meta.label} connection`}?`}
                confirmLabel="Remove and delete data"
                busy={disconnecting}
                error={saveError?.message}
                onConfirm={remove}
                onCancel={() => setConfirmRemove(false)}
              >
                <p>
                  This connection and every item imported through it are deleted. Your AI apps will no longer find them. Other {meta.label} connections
                  stay. This can&apos;t be undone. To keep the items, choose Disconnect instead.
                </p>
              </ConfirmDialog>
            </section>
          )}
        </div>

        {connector ? (
          <RoutingSection label={meta.label} connector={connector} />
        ) : (
          <aside className="soft-panel p-5 flex flex-col gap-2" aria-labelledby="routing-pending">
            <h2 id="routing-pending" className="section-title">
              Where will my items go?
            </h2>
            <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
              New items are saved in your Personal memory space, where only you can read them. After saving you can choose a shared space instead.
            </p>
          </aside>
        )}
      </div>
    </div>
  );
}
