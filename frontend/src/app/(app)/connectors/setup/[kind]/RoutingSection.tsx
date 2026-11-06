"use client";

import VaultChip, { vaultTone } from "@/components/VaultChip";
import { useState } from "react";
import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";
import Select from "@/components/Select";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import SyncMark from "@/components/bits/SyncMark";
import Tooltip from "@/components/bits/Tooltip";
import { useVaults } from "@/lib/queries/vaults";
import { useMoveConnectorData, useUpdateConnector } from "@/lib/queries/connectors";
import type { Connector, MoveReport, Routing, Vault } from "@/lib/types";

const ICON = { size: 18, strokeWidth: 1.75 } as const;

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
  const parts = [plural(r.records, "item", "items"), plural(r.memories, "fact", "facts"), plural(r.relations, "relation", "relations")];
  const left = r.left ? ` ${plural(r.left, "fact stays", "facts stay")} in a memory space you have left.` : "";
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
  const [confirmMove, setConfirmMove] = useState(false);

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
      setError(failure(e, "Could not save the routing.", " Check every rule names a value and a memory space you belong to."));
    } finally {
      setSaving(false);
    }
  }

  async function moveExisting() {
    setConfirmMove(false);
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
    <section className="soft-panel p-5 flex flex-col gap-4" aria-labelledby="routing-heading">
      <div className="flex flex-col gap-1.5">
        <h2 id="routing-heading" className="section-title">
          Where will my items go?
        </h2>
        <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
          {(() => {
            const dest = vaults?.find((v) => v.id === (routing.default_vault ?? personal?.id));
            const shared = dest && !(dest.kind === "personal" && dest.role === "owner");
            return dest ? (
              <>
                New {label} items are saved in <VaultChip tone={vaultTone(dest.kind)} name={vaultLabel(dest)} />.{" "}
                {shared ? "Its members can read them." : "Only you can read them."}
              </>
            ) : (
              `New ${label} items are saved in one memory space, and only its members can read them.`
            );
          })()}{" "}
          Changing this affects future updates only; moving items you already imported is a separate step below.
        </p>
      </div>
      <div className="ledger flex flex-col hairline-rows">
        <div className="p-4 flex flex-col gap-1.5">
          <label htmlFor="routing-destination" className="text-[14px] font-medium">
            Destination space
          </label>
          {vaults ? (
            <Select
              id="routing-destination"
              className="h-10 text-[14px] w-full"
              value={routing.default_vault ?? personal?.id ?? ""}
              onChange={(v) => edit({ ...routing, default_vault: v })}
              options={vaultOptions}
            />
          ) : (
            <span className="skeleton h-10 w-full" aria-hidden />
          )}
          <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Items no rule matches go here.
          </span>
        </div>

        <div className="p-4 flex flex-col gap-3">
          <div className="flex flex-col gap-0.5">
            <span className="text-[14px] font-medium">Advanced: rules</span>
            <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              {routing.rules.length ? "Send matching items to another space. Checked top to bottom; the first match wins." : `No rules: every ${label} item goes to the destination space.`}
            </span>
          </div>
          {routing.rules.length > 0 && (
            <ol className="flex flex-col gap-2">
              {routing.rules.map((rule, i) => {
                const n = i + 1;
                const bad = empty.includes(i);
                return (
                  <li key={i} className="flex flex-col gap-1">
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="text-[13px] w-4 text-right" style={{ color: "var(--ink-dim)" }} aria-hidden>
                        {n}
                      </span>
                      <Select
                        aria-label={`Rule ${n} field`}
                        className="h-10 text-[14px] w-36"
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
                        className="field h-10 px-3 text-[14px] flex-1 min-w-[120px]"
                        style={bad ? { borderColor: "var(--critical)" } : undefined}
                        placeholder={fieldInfo(rule.field).placeholder}
                        value={rule.value}
                        onChange={(e) => setRule(i, { value: e.target.value })}
                      />
                      <span className="text-[13px]" style={{ color: "var(--ink-dim)" }} aria-hidden>
                        to
                      </span>
                      <Select
                        aria-label={`Rule ${n} space`}
                        className="h-10 text-[14px] w-40"
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
                      <p id={`rule-err-${i}`} className="text-[13px] pl-6" style={{ color: "var(--critical)" }}>
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
          <div className="px-4 py-3">
            <ErrorLine error={error} />
          </div>
        )}

        <div className="px-4 py-3 flex items-center gap-3 flex-wrap">
          <button type="button" className="btn btn-primary min-w-[112px]" onClick={save} disabled={saving || draft === null} aria-busy={saving || undefined}>
            {savedFlash ? "Saved" : saving ? "Saving…" : "Save routing"}
          </button>
          <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Applies to items imported from now on.
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-2 pt-1" style={{ borderTop: "1px solid var(--border)" }}>
        <h3 className="text-[15px] font-medium pt-3 tracking-[-0.015em]">Move previously imported items</h3>
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {draft !== null
            ? "Save the routing first."
            : `Re-file the ${label} items already imported, with the facts drawn from them, to where the current routing sends them.`}
        </p>
        <button type="button" className="btn self-start" onClick={() => setConfirmMove(true)} disabled={moving || draft !== null}>
          <SyncMark status={moving ? "running" : moveResult ? (moveResult.ok ? "done" : "failed") : "idle"} size={18} />
          {moving ? "Moving…" : "Move existing data"}
        </button>
      </div>
      <ConfirmDialog
        open={confirmMove}
        title={`Move ${label} items to where your routing sends them?`}
        confirmLabel="Move now"
        onConfirm={moveExisting}
        onCancel={() => setConfirmMove(false)}
      >
        <p>
          Items already imported from {label}, and the memories drawn from them, move to the memory spaces your current routing names. Members of
          those spaces will be able to read them, and members of the spaces they leave will not.
        </p>
        <p style={{ color: "var(--ink-dim)" }}>The originals in {label} stay unchanged.</p>
      </ConfirmDialog>
      {moveResult && (
        <p role="status" className="text-[14px] fade-in" style={{ color: moveResult.ok ? "var(--good)" : "var(--critical)" }}>
          {moveResult.text}
        </p>
      )}
    </section>
  );
}
