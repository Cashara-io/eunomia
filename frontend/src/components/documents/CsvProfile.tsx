"use client";

// An uploaded CSV on the document view: what each column holds, and once an owner approved its
// mapping (Kinds and links, Auto-map), a button that imports its rows as entities with typed values.
import { useState } from "react";
import { RowsPlusBottomIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import Duo from "@/components/ui/Duo";
import type { ImportOut } from "@/lib/gen";
import { useDocumentProfile, useImportRows } from "@/lib/queries/documents";

const TYPE: Record<string, string> = {
  text: "Text",
  number: "Number",
  integer: "Whole number",
  money: "Money",
  percent: "Percent",
  bool: "Yes or no",
  date: "Date",
  datetime: "Date and time",
  enum: "One of a list",
  id: "Key",
};

const words = (name: string) => name.replace(/_/g, " ");
const percent = (share: number) => `${Math.round(share * 100)}%`;

function LinkTargets({ result, preview = false }: { result: Pick<ImportOut, "reused" | "ambiguous" | "skipped">; preview?: boolean }) {
  return (
    <div className="flex flex-col gap-1 text-[13px]">
      {result.reused.length > 0 && (
        <p data-testid="import-reused">
          {preview ? "Will link to" : "Linked to"} {result.reused.length === 1 ? "an entity" : "entities"} you already have instead of adding new ones:{" "}
          {result.reused.map((r) => `${r.name} (${words(r.kind)})`).join(", ")}.
        </p>
      )}
      {result.ambiguous.length > 0 && (
        <p data-testid="import-ambiguous" style={{ color: "var(--ink-dim)" }}>
          {preview ? "Will skip" : "Not linked"}, because each names more than one entity: {result.ambiguous.map((a) => `${a.name} (${a.kinds.map(words).join(" and ")})`).join(", ")}.
          Merge the duplicates, then import again.
        </p>
      )}
      {result.skipped && result.skipped.length > 0 && (
        <p data-testid="import-skipped" style={{ color: "var(--ink-dim)" }}>
          {preview ? "Will skip" : "Not linked"}: {result.skipped.map((r) => `${r.name}: ${r.reason}`).join("; ")}.
        </p>
      )}
    </div>
  );
}

function Imported({ out }: { out: ImportOut }) {
  const n = (count: number, one: string, many: string) => (
    <>
      <span className="font-mono">{count}</span> {count === 1 ? one : many}
    </>
  );
  return (
    <div role="status" className="flex flex-col gap-1 text-[13px]">
      <p>
        Imported {n(out.rows, "row", "rows")} as {words(out.kind)}: {n(out.created, "new", "new")}, {n(out.updated, "updated", "updated")},{" "}
        {n(out.values, "value", "values")}.
      </p>
      {(out.linked > 0 || out.targets_created > 0) && (
        <p>
          Linked {n(out.linked, "row", "rows")}, adding {n(out.targets_created, "new entity", "new entities")} they name.
        </p>
      )}
      <LinkTargets result={out} />
      {out.bad_count > 0 && (
        <>
          <p style={{ color: "var(--ink-dim)" }}>
            {n(out.bad_count, "cell was", "cells were")} left out. The rest was imported.
          </p>
          <ul className="flex flex-col gap-0.5 text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {out.bad.slice(0, 5).map((b) => (
              <li key={`${b.line} ${b.column ?? ""}`}>
                Line <span className="font-mono">{b.line}</span>
                {b.column ? `, ${b.column}` : ""}: {b.error}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

export default function CsvProfile({ documentId }: { documentId: string }) {
  const profile = useDocumentProfile(documentId, true);
  const importRows = useImportRows();
  const [nameColumn, setNameColumn] = useState<string | null>(null);
  const [out, setOut] = useState<ImportOut | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const p = profile.data;

  if (profile.isError) return <ErrorLine error={failure(profile.error, "Could not read the columns of this file.")} />;
  if (!p) {
    return (
      <div className="flex flex-col gap-2" aria-busy="true" aria-label="Loading columns">
        <span className="skeleton h-4 w-32" />
        <span className="skeleton h-20 w-full" />
      </div>
    );
  }
  const plan = p.import;
  const name = nameColumn ?? plan.name_column ?? p.columns[0];

  async function run() {
    if (!plan.kind) return;
    setError(null);
    setOut(null);
    try {
      setOut(await importRows.mutateAsync({ id: documentId, kind: plan.kind, name_column: name }));
    } catch (e) {
      setError(failure(e, "Could not import the rows. Try again."));
    }
  }

  return (
    <div className="flex flex-col gap-3 min-w-0" data-testid="csv-profile">
      <h3 className="section-title flex items-center gap-2">
        Columns <span className="label font-mono">{p.columns.length}</span>
        <span className="label">
          · <span className="font-mono">{p.rows}</span> {p.rows === 1 ? "row" : "rows"}
        </span>
      </h3>
      <div className="overflow-x-auto">
        <table className="data-table w-full table-fixed">
          <colgroup>
            <col className="w-[30%]" />
            <col className="w-[24%]" />
            <col />
            <col className="w-[64px]" />
          </colgroup>
          <thead>
            <tr>
              <th scope="col">Column</th>
              <th scope="col">Type</th>
              <th scope="col">Top values</th>
              <th scope="col" className="text-right">
                Empty
              </th>
            </tr>
          </thead>
          <tbody>
            {p.profile.fields.map((f) => {
              const col = plan.columns.find((c) => c.column === f.path);
              const mapped = col?.property ?? (col?.link ? col.link.replace(/_/g, " ") : null);
              return (
                <tr key={f.path}>
                  <td>
                    <span className="font-medium block truncate" title={f.path}>
                      {f.path}
                    </span>
                    {mapped && (
                      <span className="label block truncate" title={`→ ${words(mapped)}`} data-testid="csv-mapped">
                        → {words(mapped)}
                      </span>
                    )}
                  </td>
                  <td>{TYPE[f.inferred_type] ?? f.inferred_type}</td>
                  <td className="truncate" style={{ color: "var(--ink-dim)" }} title={f.top_values.map((v) => v.value).join(", ")}>
                    {f.top_values.slice(0, 3).map((v) => v.value).join(", ")}
                  </td>
                  <td className="text-right font-mono">{percent(f.null_rate)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {!plan.kind && (
        <p className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
          To import these rows as entities, open this file&apos;s memory space, run Auto-map on it in Kinds and links, and approve what it
          suggests. Then come back here to import them.
        </p>
      )}
      {plan.kind && !plan.ready && (
        <p className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
          Mapped to {words(plan.kind)}. Import rows works once an owner approves {words(plan.kind)} and at least one of its properties in Kinds and links.
        </p>
      )}
      {plan.kind && plan.ready && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[13px]">Name each {words(plan.kind)} by</span>
          <Select
            className="h-8 text-[13px] w-[180px]"
            aria-label="Column that names each row"
            value={name}
            options={p.columns.map((c) => ({ value: c, label: c }))}
            onChange={setNameColumn}
            disabled={importRows.isPending}
          />
          <button type="button" className="btn btn-sm" onClick={run} disabled={importRows.isPending}>
            <Duo icon={RowsPlusBottomIcon} tone="none" />
            {importRows.isPending ? "Importing…" : "Import rows"}
          </button>
        </div>
      )}
      {plan.ready && !out && <LinkTargets result={plan} preview />}
      {out && <Imported out={out} />}
      {error && <ErrorLine error={error} />}
    </div>
  );
}
