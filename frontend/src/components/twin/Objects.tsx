"use client";
import { useState } from "react";
import Link from "next/link";
import Select from "@/components/Select";
import ErrorLine, { failure } from "@/components/ErrorLine";
import { useObjects, useObjectKinds } from "@/lib/queries/objects";
import { useTwinParams } from "./controls";
import ObjectProfile, { formatted, objectHref } from "./ObjectProfile";
import { words as plainWords } from "./DefinitionRows";
const words = (value: string) => {
  const text = plainWords(value);
  return text.charAt(0).toUpperCase() + text.slice(1);
};

export default function Objects({ space }: { space: string }) {
  const { params, update } = useTwinParams();
  const kinds = useObjectKinds(space);
  const kind =
    params.get("kind") ||
    kinds.data?.kinds.find((k) => k.entities > 0)?.kind ||
    kinds.data?.kinds[0]?.kind ||
    "";
  const query = useObjects(space, params, kind);
  const [hidden, setHidden] = useState<string[] | null>(null);
  const [columnsOpen, setColumnsOpen] = useState(false);
  const data = query.data;
  const page = Math.max(1, Number(params.get("page")) || 1);
  const selected = params.get("selected");
  const hiddenColumns =
    hidden ?? data?.columns.slice(3).map((c) => c.name) ?? [];
  const columns =
    data?.columns.filter((c) => !hiddenColumns.includes(c.name)) ?? [];
  return (
    <div className="flex flex-col gap-4 min-w-0 text-[13px]">
      {kinds.isError && (
        <ErrorLine
          error={failure(
            kinds.error,
            "Could not load object kinds. Use Refresh to try again.",
          )}
        />
      )}
      <div>
        <h2 className="text-[22px] font-medium">Objects</h2>
        <p className="text-[var(--ink-dim)] mt-2">
          Real objects and their latest approved properties. Missing values stay
          missing.
        </p>
      </div>
      <div className="flex flex-wrap gap-3 items-end">
        <div className="flex flex-col gap-1">
          <span className="label">Kind</span>
          <Select
            aria-label="Object kind"
            value={kind}
            options={(kinds.data?.kinds ?? []).map((k) => ({
              value: k.kind,
              label: words(k.kind),
            }))}
            onChange={(k) => {
              setHidden(null);
              update({
                kind: k,
                page: "1",
                selected: "",
                sort: "",
                filter: "",
                op: "",
                value: "",
              });
            }}
          />
        </div>
        <label className="flex-1 min-w-40 flex flex-col gap-1">
          <span className="label">Search objects</span>
          <input
            className="field w-full px-3 min-h-10"
            type="search"
            value={params.get("search") ?? ""}
            onChange={(e) =>
              update({ search: e.target.value, selected: "", page: "1" }, true)
            }
          />
        </label>
        <button
          className="btn"
          aria-expanded={columnsOpen}
          onClick={() => setColumnsOpen(!columnsOpen)}
        >
          Columns
        </button>
      </div>
      {columnsOpen && (
        <fieldset className="ledger p-4 flex flex-wrap gap-4">
          <legend className="px-2">Visible columns</legend>
          {data?.columns.map((c) => (
            <label key={c.name} className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={!hiddenColumns.includes(c.name)}
                onChange={(e) =>
                  setHidden(
                    e.target.checked
                      ? hiddenColumns.filter((n) => n !== c.name)
                      : [...hiddenColumns, c.name],
                  )
                }
              />
              {words(c.name.replace(/^link:/, ""))}
            </label>
          ))}
        </fieldset>
      )}
      <details className="ledger p-3">
        <summary className="cursor-pointer">
          Filters {params.get("op") ? "(1 active)" : ""}
        </summary>
        <div className="flex flex-wrap items-end gap-3 mt-3">
          <div>
            <span className="label">Property</span>
            <Select
              aria-label="Filter property"
              value={params.get("filter") ?? ""}
              options={[
                { value: "", label: "Choose property" },
                ...(data?.columns
                  .filter((c) => c.datatype !== "link")
                  .map((c) => ({
                    value: c.name,
                    label: words(c.name.replace(/^link:/, "")),
                  })) ?? []),
              ]}
              onChange={(v) => update({ filter: v, op: "", value: "" })}
            />
          </div>
          <div>
            <span className="label">Condition</span>
            <Select
              aria-label="Filter condition"
              value={params.get("op") ?? ""}
              options={[
                { value: "", label: "No filter" },
                { value: "eq", label: "Equals" },
                { value: "gte", label: "At least" },
                { value: "lte", label: "At most" },
                { value: "missing", label: "Missing" },
              ]}
              onChange={(v) => update({ op: v })}
              disabled={!params.get("filter")}
            />
          </div>
          <label>
            <span className="label">Value</span>
            <input
              className="field px-3 w-full min-h-10"
              value={params.get("value") ?? ""}
              onChange={(e) => update({ value: e.target.value }, true)}
              disabled={params.get("op") === "missing"}
            />
          </label>
          <button
            className="btn btn-sm"
            onClick={() => update({ filter: "", op: "", value: "" })}
          >
            Clear filters
          </button>
        </div>
      </details>
      {query.isError ? (
        <ErrorLine
          error={failure(
            query.error,
            "Could not load objects. Check filters and try again.",
          )}
        />
      ) : !data ? (
        <p role="status">
          {kinds.data?.kinds.length === 0
            ? "No objects or kinds recorded in this space."
            : "Loading objects…"}
        </p>
      ) : (
        <div className="flex gap-5 items-start min-w-0">
          <div className="min-w-0 flex-1">
            <div
              className="ledger overflow-x-auto max-w-full"
              role="region"
              aria-label="Objects table"
              tabIndex={0}
            >
              <table
                className="data-table w-full whitespace-nowrap"
                aria-label={`${words(kind)} objects`}
              >
                <thead>
                  <tr>
                    {[
                      { name: "name", datatype: "text", unit: null },
                      ...columns,
                    ].map((c) => (
                      <th
                        key={c.name}
                        scope="col"
                        className={
                          c.name === "name"
                            ? "px-4 sticky left-0 bg-[var(--surface)]"
                            : "px-4"
                        }
                        aria-sort={
                          (params.get("sort") ?? "name") === c.name
                            ? params.get("descending") === "true"
                              ? "descending"
                              : "ascending"
                            : "none"
                        }
                      >
                        <button
                          disabled={c.datatype === "link"}
                          className="py-3"
                          onClick={() =>
                            update({
                              page: "1",
                              sort: c.name,
                              descending: String(
                                (params.get("sort") ?? "name") === c.name &&
                                  params.get("descending") !== "true",
                              ),
                              selected: "",
                            })
                          }
                        >
                          {words(c.name.replace(/^link:/, ""))}
                        </button>
                      </th>
                    ))}
                    <th scope="col" className="px-4">
                      Sources
                    </th>
                    <th scope="col" className="hidden xl:table-cell px-3">
                      <span className="sr-only">Inspect</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {data.rows.map((row) => (
                    <tr
                      key={row.id}
                      className={
                        selected === row.id ? "bg-[var(--accent-soft)]" : ""
                      }
                    >
                      <th
                        scope="row"
                        className="px-4 text-left font-normal sticky left-0 bg-[var(--surface)]"
                      >
                        <Link
                          className="link inline-block max-w-44 truncate align-middle"
                          title={row.name}
                          href={objectHref(row.id, space)}
                        >
                          {row.name}
                        </Link>
                      </th>
                      {columns.map((c) => (
                        <td
                          key={c.name}
                          className={`px-4 ${row.values[c.name]?.value === null ? "text-[var(--ink-faint)]" : ["number", "integer", "money", "percent", "date", "datetime"].includes(c.datatype) ? "font-mono" : ""}`}
                        >
                          {formatted(row.values[c.name]?.value, {
                            ...c,
                            unit: row.values[c.name]?.unit ?? c.unit,
                          })}
                        </td>
                      ))}
                      <td className="px-4 font-mono">
                        <Link
                          className="link"
                          href={`${objectHref(row.id, space)}&section=Sources`}
                        >
                          {row.source_count}
                        </Link>
                      </td>
                      <td className="hidden xl:table-cell px-3">
                        <button
                          className="btn btn-sm btn-ghost"
                          onClick={() => update({ selected: row.id })}
                        >
                          Inspect<span className="sr-only"> {row.name}</span>
                        </button>
                      </td>
                    </tr>
                  ))}
                  {data.rows.length === 0 && (
                    <tr>
                      <td colSpan={columns.length + 3} className="p-4">
                        {data.total === 0 ? (
                          "No objects match these filters."
                        ) : (
                          <>
                            This page has no objects.{" "}
                            <button
                              className="link"
                              onClick={() =>
                                update({ page: "1", selected: "" })
                              }
                            >
                              Return to first page
                            </button>
                          </>
                        )}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
            <nav
              aria-label="Object pages"
              className="flex flex-wrap justify-between items-center gap-3 mt-4"
            >
              <span className="font-mono">
                {data.rows.length === 0 ? 0 : data.offset + 1} to{" "}
                {data.rows.length === 0
                  ? 0
                  : Math.min(data.offset + data.limit, data.total)}{" "}
                of {data.total.toLocaleString("en-AU")}
              </span>
              <div className="flex gap-2">
                <button
                  className="btn btn-sm"
                  disabled={page === 1}
                  onClick={() =>
                    update({ page: String(page - 1), selected: "" })
                  }
                >
                  Previous
                </button>
                <button
                  className="btn btn-sm"
                  disabled={data.offset + data.limit >= data.total}
                  onClick={() =>
                    update({ page: String(page + 1), selected: "" })
                  }
                >
                  Next
                </button>
              </div>
            </nav>
          </div>
          {selected && (
            <aside
              className="hidden xl:block w-72 shrink-0 ledger p-4"
              aria-label="Object inspector"
            >
              <button
                className="btn btn-ghost btn-sm mb-3"
                onClick={() => update({ selected: "" })}
              >
                Close inspector
              </button>
              <ObjectProfile
                key={selected}
                space={space}
                id={selected}
                compact
              />
            </aside>
          )}
        </div>
      )}
    </div>
  );
}
