"use client";

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import Select from "@/components/Select";
import { useSources } from "@/lib/queries/sources";
import { useVaults } from "@/lib/queries/vaults";
import { sourceHealth } from "@/lib/sourceState";
import { relativeTime } from "@/lib/connectorMeta";
import { API_URL } from "@/lib/api";

const labels: Record<string, string> = { googledrive: "Google Drive", gmail: "Gmail", googlecalendar: "Google Calendar", timelinesai: "Personal WhatsApp (TimelinesAI)", whatsapp: "WhatsApp Business", github: "Obsidian via GitHub" };
const selection = z.object({ toolkit: z.string(), account: z.string(), folder: z.string().default(""), repo: z.string().default(""), reference: z.string().default("main"), phone_number_id: z.string().default("") });
const statusSchema = z.object({ connected: z.boolean(), account: z.string(), accounts: z.array(z.object({ toolkit: z.string(), id: z.string(), label: z.string(), status: z.string() })), apps: z.array(selection), vault: z.string().nullable(), enabled: z.boolean(), health: z.string().nullable() });
type Selection = z.infer<typeof selection>;
async function request(path: string, body?: unknown) {
  const response = await fetch(`${API_URL}/api/connectors/composio/${path}`, { credentials: "include", method: body === undefined ? "GET" : "POST", headers: body === undefined ? {} : { "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const value: unknown = await response.json();
  if (!response.ok) throw new Error(z.object({ detail: z.string() }).safeParse(value).data?.detail ?? "Composio could not complete this action. Try again.");
  return value;
}
export default function ComposioCard() {
  const cache = useQueryClient();
  const status = useQuery({ queryKey: ["composio"], queryFn: async () => statusSchema.parse(await request("status")), staleTime: 15000, retry: false });
  const sources = useSources();
  const vaults = useVaults();
  const [apps, setApps] = useState<Selection[] | null>(null);
  const [vault, setVault] = useState<string | null>(null);
  const [toolkit, setToolkit] = useState("googledrive");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const data = status.data;
  const chosen = apps ?? data?.apps ?? [];
  const source = sources.data?.find((s) => s.key === "composio");
  const health = sourceHealth(source);
  async function act(run: () => Promise<void>) {
    setBusy(true); setError(""); setMessage("");
    try { await run(); } catch (error) { setError(error instanceof Error ? error.message : "Composio could not complete this action."); } finally { setBusy(false); }
  }
  function update(index: number, patch: Partial<Selection>) { setApps(chosen.map((s, i) => i === index ? { ...s, ...patch } : s)); }
  return <section className="ledger" aria-labelledby="composio-heading">
    <div className="px-5 py-4 flex flex-wrap items-center gap-3 border-b">
      <div className="flex-1"><h2 id="composio-heading" className="section-title">Composio</h2><p className="label mt-1">Connect your own account, then choose the apps Eunomia can read.</p></div>
      {data?.connected ? <><span className="text-[13px]">Signed in as {data.account}</span><button className="btn btn-sm" disabled={busy} onClick={() => act(async () => { const result = z.object({ message: z.string() }).parse(await request("disconnect", {})); setMessage(result.message); setApps(null); await status.refetch(); })}>Disconnect</button></> : <button className="btn btn-primary" disabled={busy || status.isPending} onClick={() => act(async () => { const { url } = z.object({ url: z.string().url() }).parse(await request("begin", {})); window.location.assign(url); })}>Connect Composio</button>}
    </div>
    {data?.connected && <div className="px-5 py-4 flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3"><Select aria-label="App to connect" value={toolkit} onChange={setToolkit} options={Object.entries(labels).map(([value, label]) => ({ value, label }))} /><button className="btn" disabled={busy} onClick={() => act(async () => { const { url } = z.object({ url: z.string().url() }).parse(await request("manage", { toolkit })); window.open(url, "_blank", "noopener,noreferrer"); })}>Connect another app</button><button className="btn btn-ghost" disabled={busy} onClick={() => status.refetch()}>Refresh apps</button></div>
      <div className="overflow-x-auto"><table className="data-table"><thead><tr><th>App account</th><th>Status</th><th>Sync</th></tr></thead><tbody>{data.accounts.map((account) => {
        const selected = chosen.some((s) => s.toolkit === account.toolkit && s.account === account.id);
        return <tr key={`${account.toolkit}:${account.id}`}><td>{labels[account.toolkit] ?? account.toolkit}<span className="block label">{account.label}</span></td><td>{account.status}</td><td><input type="checkbox" style={{ accentColor: "var(--accent)" }} aria-label={`Sync ${labels[account.toolkit] ?? account.toolkit}: ${account.label}`} checked={selected} disabled={(!selected && account.status.toUpperCase() !== "ACTIVE") || busy} onChange={(e) => setApps(e.target.checked ? [...chosen, selection.parse({ toolkit: account.toolkit, account: account.id })] : chosen.filter((s) => !(s.toolkit === account.toolkit && s.account === account.id)))} /></td></tr>;
      })}{!data.accounts.length && <tr><td colSpan={3}>No apps connected yet. Connect an app in Composio, then refresh this list.</td></tr>}</tbody></table></div>
      {chosen.map((s, i) => <div key={`${s.toolkit}:${s.account}`} className="flex flex-wrap gap-3 items-center">
        {s.toolkit === "googledrive" && <label className="flex flex-col gap-1 label">Drive folder ID, blank for all accessible supported files<input className="field" aria-label={`Drive folder ${i + 1}`} value={s.folder} onChange={(e) => update(i, { folder: e.target.value })} /></label>}
        {s.toolkit === "github" && <><label className="flex flex-col gap-1 label">Obsidian repository<input className="field" placeholder="owner/repository" value={s.repo} onChange={(e) => update(i, { repo: e.target.value })} /></label><label className="flex flex-col gap-1 label">Branch or ref<input className="field" value={s.reference} onChange={(e) => update(i, { reference: e.target.value })} /></label><label className="flex flex-col gap-1 label">Vault folder, blank for repository root<input className="field" value={s.folder} onChange={(e) => update(i, { folder: e.target.value })} /></label></>}
        {s.toolkit === "whatsapp" && <label className="flex flex-col gap-1 label">Business phone number ID<input className="field" value={s.phone_number_id} onChange={(e) => update(i, { phone_number_id: e.target.value })} /></label>}
      </div>)}
      <div className="flex flex-wrap items-center gap-3"><label htmlFor="composio-vault" className="label">Target vault</label><Select id="composio-vault" value={vault ?? data.vault ?? ""} onChange={setVault} options={[{ value: "", label: "Personal vault" }, ...(vaults.data ?? []).map((v) => ({ value: v.id, label: v.name }))]} /><button className="btn btn-primary" disabled={busy} onClick={() => act(async () => { await request("selection", { apps: chosen, vault: (vault ?? data.vault) || null }); setMessage("App choices saved."); setApps(null); await status.refetch(); await cache.invalidateQueries({ queryKey: ["connectors"] }); })}>Save app choices</button><button className="btn" disabled={busy || !data.enabled} onClick={() => act(async () => { const response = await fetch(`${API_URL}/api/sources/composio/sync`, { method: "POST", credentials: "include" }); if (!response.ok) throw new Error("Sync could not start. Check your connection and try again."); setMessage("Sync started. Saved progress resumes automatically."); await sources.refetch(); })}>Sync now</button></div>
      <p className="label">{health.label}. Last completed sync: {relativeTime(source?.sync_status.last_ok ?? null)}. {source?.sync_status.last_report?.message ?? source?.sync_status.last_error}</p>
      <p className="label">Drive: text, Markdown, JSON, CSV and PDF, plus Docs and Sheets exports. Folder selection reads direct children. Calendar reads your primary calendar. Personal WhatsApp needs a separate TimelinesAI service and reads text messages. WhatsApp Business reads delivery history, not a personal inbox or message bodies. Obsidian reads Markdown already pushed to GitHub, with front matter and wikilinks. A Drive folder can also hold exported vault files.</p>
      <p className="label">Disconnect deletes local tokens. If Composio does not offer remote revocation, remove Eunomia&apos;s session in Composio too. Imported records stay in Eunomia.</p>
    </div>}
    {(error || status.error || data?.health) && <p role="alert" className="px-5 py-3" style={{ color: "var(--critical)" }}>{error || status.error?.message || data?.health}</p>}
    {message && <p role="status" className="px-5 py-3 label">{message}</p>}
  </section>;
}
