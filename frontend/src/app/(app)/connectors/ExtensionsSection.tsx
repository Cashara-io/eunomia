"use client";

import { useState } from "react";
import { ChevronDown, Plus, Power, RotateCw, Trash2, Webhook, X } from "lucide-react";
import type { ExtensionOut } from "@/lib/gen";
import { useCreateExtension, useDeleteExtension, useExtensions, useRotateExtensionSecret, useUpdateExtension } from "@/lib/queries/extensions";
import { useVaults } from "@/lib/queries/vaults";
import { relativeTime } from "@/lib/connectorMeta";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import StatusLabel from "@/components/ui/StatusLabel";
import Select from "@/components/Select";
import CopyButton from "@/components/bits/CopyButton";
import Tooltip from "@/components/bits/Tooltip";
import { CopyField, ICON } from "../settings/shared";

const COLS = "grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1.6fr)_72px_64px_64px_64px_64px_92px_20px]";
const NAME_MAX = 80;

/** The same delivery the docs show, with the secret filled in (or a placeholder once it can no longer be read). */
function curlExample(url: string, secret: string) {
  return `curl -X POST '${url}' \\\n  -H 'Authorization: Bearer ${secret}' \\\n  -H 'Content-Type: application/json' \\\n  -d '{"hello": "world"}'`;
}

type Minted = { title: string; ext: ExtensionOut; secret: string };

export default function ExtensionsSection() {
  const query = useExtensions();
  const vaults = useVaults().data ?? [];
  const create = useCreateExtension();
  const [creating, setCreating] = useState(false);
  const [minted, setMinted] = useState<Minted | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const rows = query.data ?? null;

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [vaultId, setVaultId] = useState("");

  async function submit() {
    setError(null);
    try {
      const res = await create.mutateAsync({ name: name.trim(), description: description.trim() || null, vault_id: vaultId || null });
      setMinted({ title: `Extension “${res.extension.name}” created`, ext: res.extension, secret: res.secret });
      setCreating(false);
      setName("");
      setDescription("");
      setVaultId("");
    } catch (e) {
      setError(failure(e, "Could not create the extension. Try again."));
    }
  }

  return (
    <section className="flex flex-col gap-3" aria-labelledby="extensions-heading">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <h2 id="extensions-heading" className="section-title">
            Custom integrations
          </h2>
          <p className="text-[14px] mt-1 max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
            For developers. An extension connects a script you run yourself, on your own computer or server. Eunomia gives it a webhook address
            that holds incoming data until the script collects it over the API. Eunomia does not host or run the script.
          </p>
        </div>
        {!creating && (
          <button type="button" className="btn" onClick={() => { setCreating(true); setMinted(null); }}>
            <Plus {...ICON} aria-hidden />
            New extension
          </button>
        )}
      </div>

      {creating && (
        <form
          className="task-panel p-5 flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <div className="flex items-start justify-between gap-3">
            <h3 className="text-[17px] font-semibold">New extension</h3>
            <button type="button" onClick={() => setCreating(false)} aria-label="Cancel" className="btn btn-ghost btn-sm btn-icon -mt-1 -mr-1">
              <X {...ICON} />
            </button>
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="ext-name" className="text-[14px] font-medium">
              Name
            </label>
            <input
              id="ext-name"
              autoFocus
              required
              className="field h-10 px-3 text-[14px] w-full"
              placeholder="e.g. Notes importer"
              maxLength={NAME_MAX}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="ext-description" className="text-[14px] font-medium">
              Description (optional)
            </label>
            <input
              id="ext-description"
              className="field h-10 px-3 text-[14px] w-full"
              placeholder="What it brings in and where it runs"
              maxLength={500}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </div>
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1.5 w-full sm:w-[240px]">
              <span className="text-[14px] font-medium">Saves to memory space</span>
              <Select
                aria-label="Target memory space"
                value={vaultId}
                onChange={setVaultId}
                options={[{ value: "", label: "My personal space" }, ...vaults.map((v) => ({ value: v.id, label: v.name }))]}
                className="h-10 text-[14px] w-full"
              />
            </div>
            <button type="submit" disabled={create.isPending || !name.trim()} className="btn btn-primary">
              <Webhook {...ICON} aria-hidden />
              {create.isPending ? "Creating…" : "Create extension"}
            </button>
          </div>
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            The memory space is where the script is expected to save; it is told with each batch it collects. Nothing is hosted here: the script runs on your side.
          </p>
        </form>
      )}

      {minted && <MintedCard minted={minted} onDismiss={() => setMinted(null)} />}

      {query.isError && <ErrorLine error={failure(query.error, "Could not load extensions.", " Reload the page to try again.")} />}
      {error && <ErrorLine error={error} />}

      <div className="ledger overflow-hidden">
        <div
          className={`hidden md:grid ${COLS} gap-x-4 px-4 h-[34px] items-center caps`}
          style={{ background: "var(--bauhaus-app-ink)", color: "var(--bauhaus-app-cream)" }}
          aria-hidden
        >
          <span>Extension</span>
          <span>Status</span>
          <span className="text-right">Pending</span>
          <span className="text-right">Collecting</span>
          <span className="text-right">Done</span>
          <span className="text-right">Failed</span>
          <span>Last delivery</span>
          <span />
        </div>
        {rows === null && !query.isError && (
          <div className="hairline-rows" aria-hidden>
            {[0, 1].map((i) => (
              <div key={i} className="flex items-center gap-3 px-4 h-12">
                <span className="skeleton w-7 h-7" />
                <span className="skeleton h-3 w-32" />
                <span className="skeleton h-3 w-20 ml-auto" />
              </div>
            ))}
          </div>
        )}
        {rows?.length === 0 && (
          <p className="px-4 py-5 text-[14px]" style={{ color: "var(--ink-dim)" }}>
            No extensions yet. Create one to get a webhook URL and a secret for a script of your own.
          </p>
        )}
        <div className="hairline-rows">
          {rows?.map((ext) => (
            <ExtensionRow
              key={ext.id}
              ext={ext}
              open={open === ext.id}
              onToggle={() => setOpen(open === ext.id ? null : ext.id)}
              onMinted={(m) => setMinted(m)}
              onError={setError}
            />
          ))}
        </div>
      </div>
    </section>
  );
}

function MintedCard({ minted, onDismiss }: { minted: Minted; onDismiss: () => void }) {
  const { ext, secret } = minted;
  return (
    <div className="task-panel p-5 flex flex-col gap-3" role="status">
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="text-[15px] font-semibold">{minted.title}</div>
          <p className="text-[14px] mt-0.5" style={{ color: "var(--ink-dim)" }}>
            Copy the secret now and store it somewhere safe. It won&apos;t be shown again; you can rotate it for a new one.
          </p>
        </div>
        <button onClick={onDismiss} aria-label="Dismiss" className="btn btn-ghost btn-sm btn-icon">
          <X {...ICON} />
        </button>
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="label">Webhook URL</span>
        <CopyField value={ext.webhook_url} />
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="label">Secret</span>
        <CopyField value={secret} reveal />
      </div>
      <Curl url={ext.webhook_url} secret={secret} />
    </div>
  );
}

function Curl({ url, secret }: { url: string; secret: string }) {
  const text = curlExample(url, secret);
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between">
        <span className="label">Try it</span>
        <CopyButton value={text} size="sm" label="Copy curl" className="btn-ghost" />
      </div>
      <pre className="field p-3 text-[12px] font-mono overflow-x-auto whitespace-pre" style={{ color: "var(--ink)" }}>
        {text}
      </pre>
    </div>
  );
}

function Count({ n, tone }: { n: number; tone?: string }) {
  return (
    <span className="font-mono text-right tabular-nums" style={{ color: n === 0 ? "var(--ink-faint)" : (tone ?? "var(--ink)") }}>
      {n.toLocaleString()}
    </span>
  );
}

function ExtensionRow({
  ext,
  open,
  onToggle,
  onMinted,
  onError,
}: {
  ext: ExtensionOut;
  open: boolean;
  onToggle: () => void;
  onMinted: (m: Minted) => void;
  onError: (e: Failure | null) => void;
}) {
  const update = useUpdateExtension();
  const rotate = useRotateExtensionSecret();
  const del = useDeleteExtension();
  const vaults = useVaults().data ?? [];
  const vaultName = ext.vault_id ? (vaults.find((v) => v.id === ext.vault_id)?.name ?? ext.vault_id) : "Personal space";
  const [confirmDelete, setConfirmDelete] = useState(false);

  async function toggleEnabled() {
    onError(null);
    try {
      await update.mutateAsync({ id: ext.id, body: { enabled: !ext.enabled } });
    } catch (e) {
      onError(failure(e, "Could not change that extension."));
    }
  }
  async function doRotate() {
    onError(null);
    try {
      const res = await rotate.mutateAsync(ext.id);
      onMinted({ title: `New secret for “${res.extension.name}”`, ext: res.extension, secret: res.secret });
    } catch (e) {
      onError(failure(e, "Could not rotate the secret."));
    }
  }
  async function doDelete() {
    setConfirmDelete(false);
    onError(null);
    try {
      await del.mutateAsync(ext.id);
    } catch (e) {
      onError(failure(e, "Could not delete that extension."));
    }
  }

  return (
    <div>
      <div className={`grid ${COLS} items-center gap-x-4 px-4 min-h-12 py-2 text-[13px]`}>
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={open}
          aria-label={`${ext.name}, ${open ? "hide" : "show"} details`}
          className="flex items-center gap-3 min-w-0 text-left"
        >
          <span
            aria-hidden
            className="inline-flex items-center justify-center shrink-0"
            style={{
              width: 28,
              height: 28,
              borderRadius: 4,
              color: "var(--ink-dim)",
              background: "var(--bauhaus-app-soft)",
            }}
          >
            <Webhook size={15} strokeWidth={1.75} />
          </span>
          <span className="min-w-0 flex flex-col">
            <span className="font-medium truncate">{ext.name}</span>
            <span className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
              <span className="md:hidden font-mono">
                {ext.pending} pending · {ext.done} done
                {ext.dead > 0 ? ` · ${ext.dead} failed` : ""}
                {" · "}
              </span>
              {ext.description || vaultName}
            </span>
          </span>
        </button>
        <span className="hidden md:flex">
          <StatusLabel tone={ext.enabled ? "good" : "neutral"}>{ext.enabled ? "On" : "Off"}</StatusLabel>
        </span>
        <span className="hidden md:block text-right">
          <Count n={ext.pending} />
        </span>
        <span className="hidden md:block text-right">
          <Count n={ext.claimed} />
        </span>
        <span className="hidden md:block text-right">
          <Count n={ext.done} />
        </span>
        <span className="hidden md:block text-right">
          <Count n={ext.dead} tone="var(--critical)" />
        </span>
        <span className="hidden md:block font-mono text-[12px]" style={{ color: "var(--ink-dim)" }} title={ext.last_delivery_at ? new Date(ext.last_delivery_at).toLocaleString() : undefined}>
          {relativeTime(ext.last_delivery_at)}
        </span>
        <button type="button" onClick={onToggle} aria-hidden tabIndex={-1} className="justify-self-end" style={{ color: "var(--ink-faint)" }}>
          <ChevronDown size={14} strokeWidth={1.75} style={{ transform: open ? "rotate(180deg)" : undefined, transition: "transform 140ms var(--ease-out)" }} />
        </button>
      </div>

      {open && (
        <div className="px-4 pb-4 pt-1 flex flex-col gap-3" style={{ background: "var(--surface)" }}>
          <div className="flex flex-col gap-1.5">
            <span className="label">Webhook URL</span>
            <CopyField value={ext.webhook_url} />
          </div>
          <Curl url={ext.webhook_url} secret="$EXTENSION_SECRET" />
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Secret ending <span className="font-mono">{ext.secret_hint}</span>, rotated {relativeTime(ext.secret_rotated_at)}. Saves to: {vaultName}. Collect batches with{" "}
            <span className="font-mono">POST /api/extensions/{ext.id}/events/claim</span> using an API token.
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className="btn btn-sm" onClick={doRotate} disabled={rotate.isPending}>
              <RotateCw {...ICON} aria-hidden />
              {rotate.isPending ? "Rotating…" : "Rotate secret"}
            </button>
            <Tooltip label={ext.enabled ? "Deliveries are refused until you switch it back on" : "Accept deliveries again"}>
              <button type="button" className="btn btn-sm" onClick={toggleEnabled} disabled={update.isPending}>
                <Power {...ICON} aria-hidden />
                {ext.enabled ? "Disable" : "Enable"}
              </button>
            </Tooltip>
            <span className="ml-auto">
              <button type="button" aria-label={`Delete ${ext.name}`} onClick={() => setConfirmDelete(true)} disabled={del.isPending} className="btn btn-sm btn-danger">
                <Trash2 {...ICON} aria-hidden />
                Delete
              </button>
            </span>
            <ConfirmDialog
              open={confirmDelete}
              danger
              title={`Delete the extension \u201c${ext.name}\u201d?`}
              confirmLabel="Delete extension"
              busy={del.isPending}
              onConfirm={doDelete}
              onCancel={() => setConfirmDelete(false)}
            >
              <p>
                Its webhook address stops accepting data and any deliveries still waiting are discarded. Your script will no longer be able to
                collect from it. Memories it already saved stay.
              </p>
            </ConfirmDialog>
          </div>
        </div>
      )}
    </div>
  );
}
