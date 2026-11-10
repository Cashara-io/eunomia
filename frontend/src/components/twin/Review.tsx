"use client";
import { haptic } from "@/lib/haptics";

import Link from "next/link";
import { useEffect, useEffectEvent, useRef, useState } from "react";
import { WarningCircleIcon } from "@phosphor-icons/react/ssr";
import styles from "./review.module.css";
import { isTyping } from "./review-model";
import { useQueryClient } from "@tanstack/react-query";
import { ApiError } from "@/lib/api";
import {
  decideVaultOntologyKind,
  decideVaultOntologyLink,
  decideVaultOntologyMetric,
  decideVaultOntologyProperty,
} from "@/lib/gen";
import { call } from "@/lib/queries/client";
import { ontologyKeys, type OntologyOut } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import ConfirmDialog from "@/components/ui/ConfirmDialog";
import StatusLabel from "@/components/ui/StatusLabel";
import Definition, { RevisionChanges } from "./Definition";
import { STATUS, words } from "./DefinitionRows";
import {
  Filter,
  Pagination,
  Search,
  statusOptions,
  typeOptions,
  useTwinParams,
} from "./controls";
import {
  definitions,
  pageNumber,
  PAGE_SIZE,
  twinHref,
  type Definition as DefinitionItem,
} from "./model";

function describe(d: DefinitionItem) {
  if (d.type === "kind") return `Kind ${words(d.name)}`;
  if (d.type === "link") return `Link type ${words(d.name)}`;
  const what = d.type === "property" ? "Property" : "Metric";
  return `${what} ${words(d.name)} on ${words(d.kind)}`;
}

type Receipt = {
  reason: string;
  action: string;
  results: { id: string; name: string; result: string }[];
};
export default function Review({
  vault,
  data,
}: {
  vault: Vault;
  data: OntologyOut;
}) {
  const { params, update } = useTwinParams();
  const qc = useQueryClient();
  const [checked, setChecked] = useState<DefinitionItem[]>([]);
  const [reason, setReason] = useState("");
  const [confirm, setConfirm] = useState<"approve" | "reject" | "merge" | null>(
    null,
  );
  const [mergeTarget, setMergeTarget] = useState("");
  const [batchBusy, setBusy] = useState(false);
  const [singleBusy, setSingleBusy] = useState(false);
  const busy = batchBusy || singleBusy;
  const queue = useRef<HTMLElement>(null);
  const returnId = useRef<string | null>(null);
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const [stale, setStale] = useState<DefinitionItem[]>([]);
  const [reviewEpoch, setReviewEpoch] = useState(0);
  const current = definitions(data);
  const [known, setKnown] = useState(() => current.map((d) => d.id));
  const newItems = current.filter(
    (d) => !known.includes(d.id) && d.item.status === "proposed",
  );
  const all = current.filter(
    (d) => known.includes(d.id) || d.item.status !== "proposed",
  );
  const status = params.get("status") ?? "proposed";
  const q = (params.get("q") ?? "").toLowerCase();
  const rows = all.filter(
    (d) =>
      (status === "all" || d.item.status === status) &&
      (!params.get("kind") ||
        params.get("kind") === "all" ||
        d.kind === params.get("kind") ||
        (d.type === "link" && d.item.to_kind === params.get("kind"))) &&
      (!params.get("type") ||
        params.get("type") === "all" ||
        d.type === params.get("type")) &&
      `${d.name} ${words(d.name)} ${d.kind} ${words(d.kind)} ${d.item.description ?? ""} ${d.item.reason ?? ""}`
        .toLowerCase()
        .includes(q),
  );
  const page = pageNumber(params.get("page"), rows.length);
  const shown = rows.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const selected = all.find((d) => d.id === params.get("selected"));
  const batch = checked.filter((d) => shown.some((row) => row.id === d.id));
  const owner = vault.role === "owner";
  const waiting = current.filter((d) => d.item.status === "proposed").length;
  function closeDetail() {
    returnId.current = selected?.id ?? null;
    update({ selected: "" });
  }
  function nextSuggestion() {
    const next = rows.find(
      (d) => d.id !== selected?.id && d.item.status === "proposed",
    );
    if (next)
      update({
        selected: next.id,
        page: String(Math.floor(rows.indexOf(next) / PAGE_SIZE) + 1),
      });
    else closeDetail();
  }
  useEffect(() => {
    if (!selected && returnId.current) {
      const id = returnId.current;
      queue.current
        ?.querySelectorAll<HTMLAnchorElement>("a[data-definition]")
        .forEach((link) => {
          if (link.dataset.definition === id)
            link.focus({ preventScroll: true });
        });
      returnId.current = null;
    }
  }, [selected]);
  const keyboard = useEffectEvent((e: KeyboardEvent) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || busy)
      return;
    if (
      e.key === "Escape" &&
      selected &&
      !document.querySelector("dialog[open]")
    ) {
      e.preventDefault();
      closeDetail();
      return;
    }
    if (isTyping(e.target) || document.querySelector("dialog[open]")) return;
    const direction =
      e.key.toLowerCase() === "j" ? 1 : e.key.toLowerCase() === "k" ? -1 : 0;
    if (!direction || rows.length === 0) return;
    e.preventDefault();
    const index = selected
      ? rows.findIndex((d) => d.id === selected.id)
      : direction > 0
        ? -1
        : rows.length;
    const next =
      rows[Math.max(0, Math.min(rows.length - 1, index + direction))];
    if (next)
      update({
        selected: next.id,
        page: String(Math.floor(rows.indexOf(next) / PAGE_SIZE) + 1),
      });
  });
  useEffect(() => {
    const handler = (e: KeyboardEvent) => keyboard(e);
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const mergeOptions =
    batch.length === 0
      ? []
      : batch.every((d) => d.type === "kind")
        ? data.kinds
            .filter(
              (k) =>
                !batch.some((d) => d.name === k.kind) &&
                k.status !== "rejected" &&
                (k.built_in || k.status === "approved" || k.entities > 0),
            )
            .map((k) => k.kind)
        : batch.every((d) => d.type === "property" && d.kind === batch[0].kind)
          ? (data.kinds.find((k) => k.kind === batch[0].kind)?.properties ?? [])
              .filter(
                (p) =>
                  p.status === "approved" &&
                  !batch.some((d) => d.name === p.name),
              )
              .map((p) => p.name)
          : batch.every((d) => d.type === "metric" && d.kind === batch[0].kind)
            ? (data.kinds.find((k) => k.kind === batch[0].kind)?.metrics ?? [])
                .filter(
                  (m) =>
                    m.status === "approved" &&
                    !batch.some((d) => d.name === m.name),
                )
                .map((m) => m.name)
            : [];
  async function decide(
    d: DefinitionItem,
    action: "approve" | "reject" | "merge",
  ) {
    const path = { vault_id: vault.id };
    const decision: { action: "approve" | "reject"; merge_into?: string } =
      action === "merge"
        ? { action: "reject", merge_into: mergeTarget }
        : { action };
    switch (d.type) {
      case "kind":
        return call(
          decideVaultOntologyKind({
            path,
            body: { kind: d.kind, decision, revision: d.item.revision, reason },
          }),
        );
      case "property":
        return call(
          decideVaultOntologyProperty({
            path,
            body: {
              kind: d.kind,
              name: d.name,
              decision,
              revision: d.item.revision,
              reason,
            },
          }),
        );
      case "metric":
        return call(
          decideVaultOntologyMetric({
            path,
            body: {
              kind: d.kind,
              name: d.name,
              decision,
              revision: d.item.revision,
              reason,
            },
          }),
        );
      case "link":
        return call(
          decideVaultOntologyLink({
            path,
            body: {
              from_kind: d.item.from_kind,
              label: d.item.label,
              to_kind: d.item.to_kind,
              decision,
              revision: d.item.revision,
              reason,
            },
          }),
        );
    }
  }
  async function runBatch() {
    if (
      !confirm ||
      busy ||
      (confirm === "merge" && !mergeOptions.includes(mergeTarget))
    )
      return;
    setBusy(true);
    const results: Receipt["results"] = [];
    const changed: DefinitionItem[] = [];
    const order = { kind: 0, property: 1, link: 2, metric: 3 };
    for (const d of [...batch].sort((a, b) => order[a.type] - order[b.type])) {
      try {
        qc.setQueryData(ontologyKeys.vault(vault.id), await decide(d, confirm));
        haptic("success");
        results.push({
          id: d.id,
          name: describe(d),
          result:
            confirm === "approve"
              ? "Approved"
              : confirm === "merge"
                ? `Merged into ${words(mergeTarget)}`
                : "Rejected without merging",
        });
      } catch (e) {
        haptic("error");
        if (e instanceof ApiError && e.code === "ontology.stale_revision")
          changed.push(d);
        results.push({
          id: d.id,
          name: describe(d),
          result:
            e instanceof Error ? e.message : "Decision failed. Try again.",
        });
      }
    }
    setReceipt({ action: confirm, reason: reason.trim(), results });
    setStale(changed);
    setChecked([]);
    setConfirm(null);
    setBusy(false);
    await qc.invalidateQueries({ queryKey: ontologyKeys.all });
    await qc.invalidateQueries({ queryKey: ["entities"] });
  }
  function download() {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify({ space: vault.id, ...receipt }, null, 2)], {
        type: "application/json",
      }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = "business-twin-review-receipt.json";
    a.click();
    URL.revokeObjectURL(url);
  }
  return (
    <>
      {newItems.length > 0 && (
        <div aria-live="polite">
          <button
            className="btn btn-sm"
            onClick={() => setKnown(current.map((d) => d.id))}
          >
            {newItems.length} new suggestions. Show in queue
          </button>
        </div>
      )}
      <header className={styles.heading}>
        <div>
          <h2 className={styles.title}>Decide with the source in sight.</h2>
          <p className={styles.subtitle}>
            {waiting} suggestions waiting.{" "}
            {owner
              ? "Read the evidence. Keep what belongs."
              : "Only an owner of this space can decide. You can inspect every suggestion and its evidence."}
          </p>
        </div>
        <p className={styles.shortcuts}>
          <span>
            <kbd>J</kbd> Next
          </span>
          <span>
            <kbd>K</kbd> Previous
          </span>
          <span>
            <kbd>Esc</kbd> Back / cancel
          </span>
        </p>
      </header>
      <div className={selected ? styles.selectedFilters : undefined}>
        <div className="hidden lg:flex gap-3">
          <ReviewFilters data={data} />
        </div>
        <details className="lg:hidden text-[13px]">
          <summary className="cursor-pointer min-h-11 py-3">
            Search and filter suggestions
          </summary>
          <div className="grid gap-3 mt-3">
            <ReviewFilters data={data} />
          </div>
        </details>
      </div>
      <div className={styles.layout}>
        <section
          ref={queue}
          aria-label="Review queue"
          className={`min-w-0 ${selected ? styles.selectedQueue : ""}`}
        >
          <div className={styles.queue}>
            <div className={styles.queuehead}>
              <h3 className="font-medium">Waiting for review</h3>
              <span>{rows.length}</span>
            </div>
            <ul>
              {shown.map((d) => (
                <li
                  key={d.id}
                  className={`${styles.queueitem} flex items-center`}
                  data-selected={selected?.id === d.id}
                >
                  {owner && d.item.status === "proposed" && (
                    <label className={styles.queuecheck}>
                      <input
                        type="checkbox"
                        aria-label={`Select ${words(d.name)}`}
                        disabled={busy}
                        checked={checked.some((row) => row.id === d.id)}
                        onChange={(e) =>
                          setChecked(
                            e.target.checked
                              ? [...checked, d]
                              : checked.filter((row) => row.id !== d.id),
                          )
                        }
                      />
                    </label>
                  )}
                  <Link
                    className={`${styles.queuelink} min-w-0 flex-1 flex flex-col`}
                    data-definition={d.id}
                    aria-current={selected?.id === d.id ? "true" : undefined}
                    onClick={(e) => {
                      if (busy) e.preventDefault();
                    }}
                    href={twinHref("/ontology/review", vault.id, {
                      ...Object.fromEntries(params.entries()),
                      selected: d.id,
                    })}
                  >
                    <strong className="[overflow-wrap:anywhere]">
                      {words(d.name)}
                    </strong>
                    <span className="label">
                      {d.type} · {words(d.kind)}
                    </span>
                    {d.item.status === "proposed" ? (
                      <span className={styles.risk}>
                        <WarningCircleIcon size={14} weight="duotone" />
                        {d.type !== "link" && d.item.merge_into
                          ? "Merge suggested"
                          : "Definition needs review"}
                      </span>
                    ) : (
                      <StatusLabel tone={STATUS[d.item.status].tone}>
                        {STATUS[d.item.status].label}
                      </StatusLabel>
                    )}
                  </Link>
                </li>
              ))}
              {shown.length === 0 && (
                <li className="p-4 text-[13px]">
                  {newItems.length > 0 && status === "proposed"
                    ? "New suggestions are ready. Use the notice above to show them."
                    : waiting === 0 && status === "proposed"
                      ? "Nothing is waiting for review. Use All statuses to inspect earlier decisions."
                      : "No definitions match. Change the search or filters."}
                </li>
              )}
            </ul>
          </div>
          <Pagination page={page} total={rows.length} />
          {owner && shown.some((d) => d.item.status === "proposed") && (
            <details className="mb-4 text-[13px]">
              <summary className="cursor-pointer min-h-11 py-3">
                Review a batch
              </summary>
              <div className="flex flex-col gap-3 mb-4">
                <label className="flex items-center gap-2 text-[13px]">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={shown
                      .filter((d) => d.item.status === "proposed")
                      .every((d) => checked.some((row) => row.id === d.id))}
                    onChange={(e) =>
                      setChecked(
                        e.target.checked
                          ? shown.filter((d) => d.item.status === "proposed")
                          : [],
                      )
                    }
                  />
                  Select suggestions on this page
                </label>
                {batch.length > 0 && (
                  <>
                    <label className="flex flex-col gap-1">
                      <span className="label">
                        Reason for this batch (optional)
                      </span>
                      <input
                        className="field px-3 min-h-11 sm:min-h-9"
                        maxLength={500}
                        value={reason}
                        onChange={(e) => setReason(e.target.value)}
                        disabled={busy}
                      />
                    </label>
                    <p
                      className="text-[12px]"
                      style={{ color: "var(--ink-dim)" }}
                    >
                      Your reason is saved with each successful decision. You
                      can also download a receipt.
                    </p>
                    <div className="flex flex-wrap gap-2">
                      <button
                        className="btn btn-primary btn-sm"
                        disabled={busy}
                        onClick={() => setConfirm("approve")}
                      >
                        Approve {batch.length} selected
                      </button>
                      <button
                        className="btn btn-sm"
                        disabled={busy}
                        onClick={() => setConfirm("reject")}
                      >
                        Reject {batch.length} selected
                      </button>
                    </div>
                    {mergeOptions.length > 0 && (
                      <div className="flex flex-wrap gap-2 items-center">
                        <label className="text-[13px]">
                          Merge selected into{" "}
                          <select
                            className="field p-2"
                            value={mergeTarget}
                            disabled={busy}
                            onChange={(e) => setMergeTarget(e.target.value)}
                          >
                            <option value="">Choose a target</option>
                            {mergeOptions.map((target) => (
                              <option key={target} value={target}>
                                {words(target)}
                              </option>
                            ))}
                          </select>
                        </label>
                        <button
                          className="btn btn-sm"
                          disabled={!mergeOptions.includes(mergeTarget) || busy}
                          onClick={() => setConfirm("merge")}
                        >
                          Reject and merge {batch.length} selected
                        </button>
                      </div>
                    )}
                  </>
                )}
              </div>
            </details>
          )}
        </section>
        <section
          aria-label="Selected suggestion"
          className={`${styles.detail} ${selected ? "" : "hidden lg:block"}`}
        >
          {selected ? (
            <>
              <button
                className={`btn btn-ghost btn-sm ${styles.back}`}
                disabled={busy}
                onClick={closeDetail}
              >
                Back to queue
              </button>
              <Definition
                key={`${selected.id}:${reviewEpoch}`}
                definition={selected}
                data={data}
                isOwner={owner}
                queueReview
                locked={batchBusy}
                onBusyChange={setSingleBusy}
                onNext={nextSuggestion}
                initialReason={
                  stale.some((d) => d.id === selected.id)
                    ? receipt?.reason
                    : undefined
                }
              />
            </>
          ) : (
            <p className="text-[13px] p-8" style={{ color: "var(--ink-dim)" }}>
              Choose a definition to inspect the proposed change, evidence,
              consequences and merge options.
            </p>
          )}
        </section>
      </div>
      {receipt && (
        <section role="status" className="ledger p-4 flex flex-col gap-3">
          <h3 className="section-title">Batch results</h3>
          <p className="text-[13px]">Recorded reason: {receipt.reason}</p>
          <ul className="text-[13px] max-h-60 overflow-auto">
            {receipt.results.map((r) => {
              const before = stale.find((d) => d.id === r.id);
              const after = current.find((d) => d.id === r.id);
              return (
                <li key={r.id} className="py-2 [overflow-wrap:anywhere]">
                  {r.name}: {r.result}
                  {before && after && (
                    <div className="flex flex-col gap-2 mt-2">
                      <RevisionChanges before={before} after={after} />
                      <button
                        aria-label={`Review changes for ${r.name}`}
                        className="btn btn-sm self-start"
                        onClick={() => {
                          setReviewEpoch((value) => value + 1);
                          update({ selected: r.id });
                        }}
                      >
                        Review changes
                      </button>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
          <button className="btn btn-sm self-start" onClick={download}>
            Download receipt
          </button>
        </section>
      )}
      <ConfirmDialog
        open={confirm !== null}
        title={`${confirm === "approve" ? "Approve" : confirm === "merge" ? "Reject and merge" : "Reject"} ${batch.length} selected definitions?`}
        confirmLabel={
          busy
            ? "Saving decisions…"
            : confirm === "approve"
              ? "Approve selected"
              : confirm === "merge"
                ? "Reject and merge selected"
                : "Reject selected"
        }
        busy={busy}
        onCancel={() => setConfirm(null)}
        onConfirm={runBatch}
      >
        <p>
          {confirm === "approve"
            ? "AI apps will be able to use the approved definitions. Required kinds are decided first. Dependencies can still refuse individual decisions."
            : confirm === "merge"
              ? `New writes under each selected definition will use ${words(mergeTarget)}. Existing objects stay where they are.`
              : "New writes under rejected definitions may be refused. No selected definition will be merged."}
        </p>
        <ul className="max-h-40 overflow-auto list-disc pl-5">
          {batch.map((d) => (
            <li key={d.id}>
              {words(d.name)} ({d.type})
            </li>
          ))}
        </ul>
        <p>Recorded reason: {reason}</p>
        <p>
          Decisions run one at a time. Some may succeed while others fail. The
          server refuses changed definitions. Review those again before
          retrying.
        </p>
      </ConfirmDialog>
    </>
  );
}

function ReviewFilters({ data }: { data: OntologyOut }) {
  return (
    <>
      <Search label="Search suggestions" />
      <Filter label="Type" param="type" options={typeOptions} />
      <Filter
        label="Kind"
        param="kind"
        options={[
          { value: "all", label: "All kinds" },
          ...data.kinds.map((k) => ({ value: k.kind, label: words(k.kind) })),
        ]}
      />
      <Filter
        label="Status"
        param="status"
        options={statusOptions}
        fallback="proposed"
      />
    </>
  );
}
