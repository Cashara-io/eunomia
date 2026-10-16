"use client";

import Select from "@/components/Select";
import { use, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import Link from "next/link";
import { ArrowRight, ChevronRight, Eye, EyeOff } from "lucide-react";
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
import RoutingSection from "./RoutingSection";

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
// per the backend scheduler (heypocket defaults to 24h there; every other
// source falls back to the scheduler's generic 900s/15min default).
const SOURCE_DEFAULT_INTERVAL: Record<string, number> = {
  up_bank: 900,
  heypocket: 86400,
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
    <nav aria-label="Breadcrumb" className="flex items-center gap-1.5 text-[12.5px]" style={{ color: "var(--ink-faint)" }}>
      <Link href="/connectors" className="hover:underline" style={{ color: "var(--ink-dim)" }}>
        Connectors
      </Link>
      <ChevronRight size={12} strokeWidth={1.75} aria-hidden />
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
        <h1 className="page-title">Unknown connector</h1>
        <div className="ledger px-5 py-4 flex items-center justify-between gap-4 flex-wrap text-[13px]">
          <span style={{ color: "var(--ink-dim)" }}>There is no connector called &ldquo;{kind}&rdquo;.</span>
          <Link href="/connectors" className="btn btn-sm">
            Browse connectors
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
      <button type="button" onClick={onToggle} aria-label={name} aria-pressed={shown} className="btn btn-ghost btn-sm w-6 px-0 -mr-1.5">
        {shown ? <EyeOff size={14} strokeWidth={1.75} aria-hidden /> : <Eye size={14} strokeWidth={1.75} aria-hidden />}
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
  const loadError = connectorsQuery.isError ? failure(connectorsQuery.error, "Could not load this connector.", " Reload the page once the backend is reachable.") : null;
  const updateConnector = useUpdateConnector();
  const testConnector = useTestConnector();
  const updateSettings = useUpdateSettings();
  const syncSource = useSyncSource();
  const [syncing, setSyncing] = useState(false);
  const [syncMessage, setSyncMessage] = useState<{ ok: boolean; text: string } | null>(null);
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
          ? { ok: false, text: `Sync failed: ${r.error}` }
          : r.status === "already_running"
            ? { ok: true, text: "A sync is already running; try again in a moment" }
            : r.status === "queued"
              ? { ok: true, text: "Sync queued. It will run as soon as a worker is free" }
              : { ok: true, text: `Synced: ${r.written ?? 0} new or changed, ${r.skipped ?? 0} unchanged` },
      );
    } catch (e) {
      setSyncMessage({ ok: false, text: `Sync failed: ${failure(e, "unknown error").message}` });
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

  async function disconnect() {
    setDisconnecting(true);
    setSaveError(null);
    try {
      if (added) {
        await deleteConnector.mutateAsync(ref);
        router.push("/connectors");
        return;
      }
      await updateConnector.mutateAsync({ connector: ref, body: { enabled: false } });
      setConfirmDisconnect(false);
      setTestResult(null);
    } catch (err) {
      setSaveError(failure(err, "Could not disconnect.", " Try again."));
    } finally {
      setDisconnecting(false);
    }
  }

  // Credentials the provider just turned down are not "Connected", whatever was saved.
  const rejected = connected && testResult !== null && !testResult.ok;
  const statusText = connector === null ? "Checking…" : isNew ? "New connection" : isDemo ? "Demo data" : rejected ? "Credentials rejected" : connected ? "Connected" : "Not connected";
  const statusTone = isDemo ? "var(--warning)" : rejected ? "var(--critical)" : connected ? "var(--good)" : "var(--ink-faint)";
  const sourceKey = meta.sourceKey;
  const override = sourceKey ? appSettings?.sync_intervals?.[sourceKey] : undefined;
  const defaultInterval = sourceKey ? SOURCE_DEFAULT_INTERVAL[sourceKey] ?? 900 : 900;

  return (
    <div className="max-w-2xl flex flex-col gap-8">
      <header className="flex flex-col gap-3">
        <Breadcrumb label={connector?.label ? `${meta.label} (${connector.label})` : meta.label} />
        <div className="flex items-center gap-3 flex-wrap">
          <ConnectorTile kind={kind} size={40} />
          <div className="flex flex-col gap-0.5 min-w-0 flex-1">
            <h1 className="page-title">{isNew ? `Add another ${meta.label} connection` : connector?.label ? `${meta.label}: ${connector.label}` : meta.label}</h1>
            <span className="text-[12.5px] flex items-center gap-1.5" style={{ color: "var(--ink-dim)" }}>
              {connector === null ? <span className="skeleton w-1.5 h-1.5" /> : <span className="dot" style={{ background: statusTone }} aria-hidden />}
              {statusText}
            </span>
          </div>
          {(connected || isDemo) && sourceKey && connector && (
            <Link href={`/connectors/${connector.source_key}`} className="btn">
              View synced data <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
            </Link>
          )}
        </div>
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {meta.description}
        </p>
      </header>

      {loadError && <ErrorLine error={loadError} />}

      {isDemo && (
        <p className="text-[13px] rounded-[10px] px-4 py-3" style={{ background: "var(--surface-raised)", color: "var(--ink-dim)" }}>
          Running on demo data. Save a real token below to switch over.
        </p>
      )}

      <form onSubmit={save} noValidate className="ledger flex flex-col">
        <div className="p-5 flex flex-col gap-5">
          <div className="flex flex-col gap-1">
            <h2 className="section-title">Credentials</h2>
            {meta.help && (
              <div className="text-[12.5px] break-words" style={{ color: "var(--ink-dim)" }}>
                {meta.help}
              </div>
            )}
          </div>

          <div className="flex flex-col gap-1.5">
            <label htmlFor="field-connection-name" className="text-[12.5px] font-medium flex items-baseline gap-2">
              Connection name
              {!isNew && <span className="label font-normal">Optional</span>}
            </label>
            <div className="field flex items-center gap-2 h-8 px-2.5" style={nameError ? { borderColor: "var(--critical)" } : undefined}>
              <input
                id="field-connection-name"
                type="text"
                autoComplete="off"
                maxLength={100}
                className="flex-1 min-w-0 bg-transparent outline-none text-[13px]"
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
              <p id="err-connection-name" className="text-[12px]" style={{ color: "var(--critical)" }}>
                {nameError}
              </p>
            ) : (
              <p id="help-connection-name" className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
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
                <label htmlFor={`field-${f.key}`} className="text-[12.5px] font-medium flex items-baseline gap-2">
                  {f.label}
                  {isOptional(f) && <span className="label font-normal">Optional</span>}
                </label>
                <div className="field flex items-center gap-2 h-8 px-2.5" style={err ? { borderColor: "var(--critical)" } : undefined}>
                  <input
                    id={`field-${f.key}`}
                    type={f.secret && !reveal ? "password" : "text"}
                    autoComplete="off"
                    spellCheck={false}
                    className="flex-1 min-w-0 bg-transparent outline-none text-[13px] font-mono"
                    placeholder={connected && f.secret ? "Saved. Paste a new value to replace it" : f.placeholder}
                    value={values[f.key] || ""}
                    onChange={(e) => setField(f.key, e.target.value)}
                    aria-invalid={err ? true : undefined}
                    aria-describedby={describedBy}
                  />
                  {f.secret && <SecretToggle shown={!!reveal} label={f.label} onToggle={() => setShown((s) => ({ ...s, [f.key]: !s[f.key] }))} />}
                </div>
                {err && (
                  <p id={describedBy} className="text-[12px]" style={{ color: "var(--critical)" }}>
                    {err}
                  </p>
                )}
              </div>
            );
          })}

          {meta.webhooks && webhookUrl !== null && (
            <div className="flex flex-col gap-1.5">
              <label htmlFor="field-webhook-url" className="text-[12.5px] font-medium">
                Webhook URL
              </label>
              <div className="flex items-center gap-2">
                <input
                  id="field-webhook-url"
                  type="text"
                  spellCheck={false}
                  className="field flex-1 min-w-0 h-8 px-2.5 text-[12.5px] font-mono"
                  value={webhookUrl}
                  onChange={(e) => setWebhookUrl(e.target.value)}
                  aria-describedby="webhook-help"
                />
                <CopyButton value={() => webhookUrl} size="sm" className="h-8 w-[84px]" />
              </div>
              <p id="webhook-help" className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
                Register this with {meta.label} so updates arrive instantly instead of on the next poll. If {meta.label} needs a public address, replace{" "}
                <span className="font-mono">{new URL(apiOrigin()).host}</span> with your tunnel host before copying.
              </p>
            </div>
          )}
        </div>

        {saveError && (
          <div className="mx-5 mb-4">
            <ErrorLine error={saveError} />
          </div>
        )}

        <div className="px-5 py-3 flex items-center gap-2 flex-wrap border-t">
          <button type="submit" disabled={saving} className="btn btn-primary min-w-[72px]">
            {savedFlash ? "Saved" : saving ? "Saving…" : "Save"}
          </button>
          <button type="button" onClick={test} disabled={testing || !(connected || isDemo)} className="btn" title={connected || isDemo ? undefined : "Save credentials first"}>
            <SyncMark status={testing ? "running" : testResult ? (testResult.ok ? "done" : "failed") : "idle"} />
            {testing ? "Testing…" : "Test connection"}
          </button>
          <span className="text-[12.5px] flex items-center gap-1.5 min-w-0" aria-live="polite">
            {testResult?.ok && "Connection works"}
          </span>
        </div>

        {testResult && !testResult.ok && (
          <div className="mx-5 mb-4">
            <ErrorLine error={testResult.error}>
              <span className="font-medium">{meta.label} rejected the connection.</span> {testResult.error?.message}{" "}
              <span style={{ color: "var(--ink-dim)" }}>Check the token is current and has the scope described above, then save it again.</span>
            </ErrorLine>
          </div>
        )}
      </form>

      {sourceKey && (
        <section className="flex flex-col gap-3" aria-labelledby="sync-heading">
          <h2 id="sync-heading" className="section-title">
            Sync
          </h2>
          <div className="ledger p-5 flex items-center justify-between gap-4 flex-wrap">
            <label htmlFor="sync-interval" className="flex flex-col gap-0.5 text-[13px]">
              Sync interval
              <span className="label">How often Eunomia pulls new records from {meta.label}.</span>
            </label>
            <div className="flex items-center gap-2">
              <span className="text-[12px] w-12 text-right" aria-live="polite" style={{ color: intervalState === "error" ? "var(--critical)" : "var(--good)" }}>
                {intervalState === "saved" ? "Saved" : intervalState === "error" ? "Not saved" : ""}
              </span>
              <Select
                id="sync-interval"
                className="h-8 text-[13px] w-44"
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
          {connected && (
            <div className="flex items-center gap-3 flex-wrap">
              <button type="button" onClick={syncNow} disabled={syncing} className="btn">
                <SyncMark status={syncing ? "running" : syncMessage ? (syncMessage.ok ? "done" : "failed") : "idle"} />
                {syncing ? "Syncing…" : "Sync now"}
              </button>
              {syncMessage && (
                <span role="status" className="text-[12.5px] font-mono break-all" style={{ color: syncMessage.ok ? "var(--good)" : "var(--critical)" }}>
                  {syncMessage.text}
                </span>
              )}
            </div>
          )}
        </section>
      )}

      {connector && <RoutingSection label={meta.label} connector={connector} />}

      {(connected || added) && (
        <section className="flex flex-col gap-3" aria-labelledby="danger-heading">
          <h2 id="danger-heading" className="section-title">
            {added ? "Remove" : "Disconnect"}
          </h2>
          <div className="ledger p-5 flex items-center justify-between gap-4 flex-wrap text-[13px]">
            <span style={{ color: "var(--ink-dim)" }}>
              {added
                ? confirmDisconnect
                  ? `Remove this ${meta.label} connection? Everything synced through it is deleted too.`
                  : `Removes this connection and everything synced through it. Other ${meta.label} connections stay.`
                : confirmDisconnect
                  ? `Stop syncing ${meta.label}? Records already synced stay in memory.`
                  : `Stops syncing. Records already synced stay in memory.`}
            </span>
            <div className="flex items-center gap-2">
              {confirmDisconnect && (
                <button type="button" className="btn btn-ghost btn-sm" onClick={() => setConfirmDisconnect(false)}>
                  Cancel
                </button>
              )}
              <button
                type="button"
                className="btn btn-danger btn-sm"
                disabled={disconnecting}
                onClick={() => (confirmDisconnect ? disconnect() : setConfirmDisconnect(true))}
              >
                {disconnecting
                  ? added
                    ? "Removing…"
                    : "Disconnecting…"
                  : confirmDisconnect
                    ? added
                      ? "Remove connection"
                      : `Disconnect ${meta.label}`
                    : added
                      ? "Remove"
                      : "Disconnect"}
              </button>
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
