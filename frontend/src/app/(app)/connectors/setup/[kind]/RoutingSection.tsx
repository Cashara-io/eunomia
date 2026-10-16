"use client";

import { useState } from "react";
import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";
import Select from "@/components/Select";
import ConfirmButton from "@/components/ConfirmButton";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import SyncMark from "@/components/bits/SyncMark";
import Tooltip from "@/components/bits/Tooltip";
import { useVaults } from "@/lib/queries/vaults";
import { useMoveConnectorData, useUpdateConnector } from "@/lib/queries/connectors";
import type { Connector, MoveReport, Routing, Vault } from "@/lib/types";

const ICON = { size: 14, strokeWidth: 1.75 } as const;

/** How each routing field reads, and an example value. */
const FIELDS: Record<string, { label: string; placeholder: string }> = {
  tag: { label: "Tag", placeholder: "work" },
  folder: { label: "Folder", placeholder: "Clients" },
  title: { label: "Title contains", placeholder: "standup" },
  repo: { label: "Repository", placeholder: "owner/name" },
  channel: { label: "Channel", placeholder: "#general or C0123ABC" },
  label: { label: "Label ID", placeholder: "IMPORTANT or Label_12" },
  calendar: { label: "Calendar", placeholder: "primary" },
  database: { label: "Database ID", placeholder: "Notion database ID" },
  page: { label: "Parent page ID", placeholder: "Notion page ID" },
  team: { label: "Team", placeholder: "Engineering" },
  project: { label: "Project", placeholder: "Home" },
};
const fieldInfo = (f: string) => FIELDS[f] ?? { label: f, placeholder: "" };

const vaultLabel = (v: Vault) => (v.kind === "personal" && v.role === "owner" ? "Personal" : v.name);

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

function moved(r: MoveReport): string {
  if (r.records === 0 && r.memories === 0 && r.relations === 0) return "Everything is already where these rules send it.";
  const parts = [plural(r.records, "record", "records"), plural(r.memories, "fact", "facts"), plural(r.relations, "relation", "relations")];
  const left = r.left ? ` ${plural(r.left, "fact stays", "facts stay")} in a vault you have left.` : "";
  return `Moved ${parts.join(", ")}.${left}`;
}

/** Where a connector's records go: a destination vault, ordered rules that send matching records
 * elsewhere, and "Move existing data" to re-file what was synced before the current routing. */
export default function RoutingSection({ label, connector }: { label: string; connector: Connector }) {
  const vaults = useVaults().data;
  const update = useUpdateConnector();
  const move = useMoveConnectorData();
  // null: showing what is saved; an object: unsaved edits
  const [draft, setDraft] = useState<Routing | null>(null);
  const [empty, setEmpty] = useState<number[]>([]);
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [moving, setMoving] = useState(false);
  const [moveResult, setMoveResult] = useState<{ ok: boolean; text: string } | null>(null);

  const saved: Routing = { default_vault: connector.routing?.default_vault, rules: connector.routing?.rules ?? [] };
  const routing = draft ?? saved;
  const fields = connector.routing_fields?.length ? connector.routing_fields : ["title"];
  const personal = vaults?.find((v) => v.kind === "personal" && v.role === "owner");
  const vaultOptions = (vaults ?? []).map((v) => ({ value: v.id, label: vaultLabel(v), hint: v.kind === "personal" && v.role === "owner" ? "default" : undefined }));

  function edit(next: Routing) {
    setDraft(next);
    setError(null);
    setMoveResult(null);
  }
  const setRule = (i: number, patch: Partial<Routing["rules"][number]>) => {
    edit({ ...routing, rules: routing.rules.map((r, j) => (j === i ? { ...r, ...patch } : r)) });
    if (patch.value !== undefined) setEmpty((e) => e.filter((j) => j !== i));
  };
  const swap = (i: number, j: number) => {
    const rules = [...routing.rules];
    [rules[i], rules[j]] = [rules[j], rules[i]];
    edit({ ...routing, rules });
    setEmpty([]);
  };
  const addRule = () => {
    const field = fields.find((f) => f !== "title") ?? fields[0];
    const target = vaults?.find((v) => v.id !== (routing.default_vault ?? personal?.id)) ?? personal;
    edit({ ...routing, rules: [...routing.rules, { field, value: "", vault: target?.id ?? "" }] });
  };
  const removeRule = (i: number) => {
    edit({ ...routing, rules: routing.rules.filter((_, j) => j !== i) });
    setEmpty([]);
  };

  async function save() {
    const blank = routing.rules.flatMap((r, i) => (r.value.trim() ? [] : [i]));
    setEmpty(blank);
    if (blank.length) {
      document.getElementById(`rule-value-${blank[0]}`)?.focus();
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const body: Routing = { rules: routing.rules.map((r) => ({ ...r, value: r.value.trim() })) };
      if (routing.default_vault && routing.default_vault !== personal?.id) body.default_vault = routing.default_vault;
      await update.mutateAsync({ connector: connector.id, body: { routing: body } });
      setDraft(null);
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 1400);
    } catch (e) {
      setError(failure(e, "Could not save the routing.", " Check every rule names a value and a vault you belong to."));
    } finally {
      setSaving(false);
    }
  }

  async function moveExisting() {
    setMoving(true);
    setMoveResult(null);
    try {
      setMoveResult({ ok: true, text: moved(await move.mutateAsync(connector.id)) });
    } catch (e) {
      setMoveResult({ ok: false, text: failure(e, "Could not move the data.", " Try again.").message });
    } finally {
      setMoving(false);
    }
  }

  return (
    <section className="flex flex-col gap-3" aria-labelledby="routing-heading">
      <div className="flex flex-col gap-1">
        <h2 id="routing-heading" className="section-title">
          Where records go
        </h2>
        <p className="text-[13px] max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
          Each record {label} syncs is filed in one vault, and only that vault&apos;s members can read it. The first rule that matches wins; anything else goes to the destination vault.
        </p>
      </div>
      <div className="ledger flex flex-col hairline-rows">
        <div className="p-5 flex items-center justify-between gap-4 flex-wrap">
          <label htmlFor="routing-destination" className="flex flex-col gap-0.5 text-[13px]">
            Destination vault
            <span className="label">Records no rule matches go here.</span>
          </label>
          {vaults ? (
            <Select
              id="routing-destination"
              className="h-8 text-[13px] w-56"
              value={routing.default_vault ?? personal?.id ?? ""}
              onChange={(v) => edit({ ...routing, default_vault: v })}
              options={vaultOptions}
            />
          ) : (
            <span className="skeleton h-8 w-56" aria-hidden />
          )}
        </div>

        <div className="p-5 flex flex-col gap-3">
          <div className="flex flex-col gap-0.5">
            <span className="text-[13px]">Rules</span>
            <span className="label">{routing.rules.length ? "Checked top to bottom." : `No rules: every ${label} record goes to the destination vault.`}</span>
          </div>
          {routing.rules.length > 0 && (
            <ol className="flex flex-col gap-2">
              {routing.rules.map((rule, i) => {
                const n = i + 1;
                const bad = empty.includes(i);
                return (
                  <li key={i} className="flex flex-col gap-1">
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="font-mono text-[12px] w-4 text-right" style={{ color: "var(--ink-faint)" }} aria-hidden>
                        {n}
                      </span>
                      <Select
                        aria-label={`Rule ${n} field`}
                        className="h-8 text-[13px] w-36"
                        value={rule.field}
                        onChange={(v) => setRule(i, { field: v })}
                        options={fields.map((f) => ({ value: f, label: fieldInfo(f).label }))}
                      />
                      <input
                        id={`rule-value-${i}`}
                        aria-label={`Rule ${n} value`}
                        aria-invalid={bad || undefined}
                        aria-describedby={bad ? `rule-err-${i}` : undefined}
                        spellCheck={false}
                        className="field h-8 px-2.5 text-[13px] flex-1 min-w-[120px]"
                        style={bad ? { borderColor: "var(--critical)" } : undefined}
                        placeholder={fieldInfo(rule.field).placeholder}
                        value={rule.value}
                        onChange={(e) => setRule(i, { value: e.target.value })}
                      />
                      <span className="text-[12.5px]" style={{ color: "var(--ink-faint)" }} aria-hidden>
                        to
                      </span>
                      <Select
                        aria-label={`Rule ${n} vault`}
                        className="h-8 text-[13px] w-40"
                        value={rule.vault}
                        onChange={(v) => setRule(i, { vault: v })}
                        options={vaultOptions}
                      />
                      <span className="flex items-center">
                        <Tooltip label="Move up">
                          <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label={`Move rule ${n} up`} disabled={i === 0} onClick={() => swap(i, i - 1)}>
                            <ArrowUp {...ICON} aria-hidden />
                          </button>
                        </Tooltip>
                        <Tooltip label="Move down">
                          <button
                            type="button"
                            className="btn btn-ghost btn-sm btn-icon"
                            aria-label={`Move rule ${n} down`}
                            disabled={i === routing.rules.length - 1}
                            onClick={() => swap(i, i + 1)}
                          >
                            <ArrowDown {...ICON} aria-hidden />
                          </button>
                        </Tooltip>
                        <Tooltip label="Remove">
                          <button type="button" className="btn btn-ghost btn-sm btn-icon" aria-label={`Remove rule ${n}`} onClick={() => removeRule(i)}>
                            <X {...ICON} aria-hidden />
                          </button>
                        </Tooltip>
                      </span>
                    </div>
                    {bad && (
                      <p id={`rule-err-${i}`} className="text-[12px] pl-6" style={{ color: "var(--critical)" }}>
                        Enter a {fieldInfo(rule.field).label.toLowerCase()} for this rule, or remove it.
                      </p>
                    )}
                  </li>
                );
              })}
            </ol>
          )}
          <div>
            <button type="button" className="btn btn-sm" onClick={addRule} disabled={!vaults}>
              <Plus {...ICON} aria-hidden />
              Add rule
            </button>
          </div>
        </div>

        {error && (
          <div className="px-5 py-3">
            <ErrorLine error={error} />
          </div>
        )}

        <div className="px-5 py-3 flex items-center gap-3 flex-wrap rounded-b-[10px]" style={{ background: "var(--surface-raised)" }}>
          <button type="button" className="btn btn-primary min-w-[112px]" onClick={save} disabled={saving || draft === null} aria-busy={saving || undefined}>
            {savedFlash ? "Saved" : saving ? "Saving…" : "Save routing"}
          </button>
          <span className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
            Applies to records synced from now on.
          </span>
        </div>
      </div>

      <div className="ledger p-5 flex items-center justify-between gap-4 flex-wrap text-[13px]">
        <span className="flex flex-col gap-0.5 max-w-[48ch]">
          Move existing data
          <span className="label">
            {draft !== null
              ? "Save the routing first."
              : `Re-file the ${label} records already synced, with the facts drawn from them, to where the current routing sends them.`}
          </span>
        </span>
        <ConfirmButton label="Move existing data" confirmLabel="Move now" onConfirm={moveExisting} disabled={moving || draft !== null} className="btn btn-sm">
          <SyncMark status={moving ? "running" : moveResult ? (moveResult.ok ? "done" : "failed") : "idle"} />
          {moving ? "Moving…" : "Move existing data"}
        </ConfirmButton>
      </div>
      {moveResult && (
        <p role="status" className="text-[12.5px] fade-in" style={{ color: moveResult.ok ? "var(--good)" : "var(--critical)" }}>
          {moveResult.text}
        </p>
      )}
    </section>
  );
}
