"use client";

import { usePathname, useSearchParams } from "next/navigation";
import Select from "@/components/Select";
import { STATUS } from "./DefinitionRows";
import { PAGE_SIZE } from "./model";

export function useTwinParams() {
  const params = useSearchParams();
  const pathname = usePathname();
  function update(values: Record<string, string>, replace = false) {
    const next = new URLSearchParams(window.location.search);
    if (["q", "search", "kind", "status", "type", "sort", "descending", "filter", "op", "value"].some(key => key in values) && !("page" in values)) next.delete("page");
    for (const [key, value] of Object.entries(values)) {
      if (value) next.set(key, value);
      else next.delete(key);
    }
    window.history[replace ? "replaceState" : "pushState"](
      null,
      "",
      `${pathname}?${next}`,
    );
  }
  return { params, update };
}

export function Search({ label = "Search definitions" }: { label?: string }) {
  const { params, update } = useTwinParams();
  return (
    <label className="flex flex-col gap-1 min-w-0 flex-1">
      <span className="label">{label}</span>
      <input
        type="search"
        className="field px-3 w-full min-h-11 sm:min-h-9"
        value={params.get("q") ?? ""}
        onChange={(e) => update({ q: e.target.value }, true)}
      />
    </label>
  );
}

export function Filter({
  label,
  param,
  options,
  fallback = "all",
}: {
  label: string;
  param: string;
  options: { value: string; label: string }[];
  fallback?: string;
}) {
  const { params, update } = useTwinParams();
  return (
    <div className="flex flex-col gap-1 min-w-0">
      <span className="label">{label}</span>
      <Select
        aria-label={label}
        value={params.get(param) ?? fallback}
        options={options}
        onChange={(v) => update({ [param]: v })}
        className="w-full sm:w-44 min-h-11 sm:min-h-9"
      />
    </div>
  );
}

export const statusOptions = [
  { value: "all", label: "All statuses" },
  ...Object.entries(STATUS).map(([value, s]) => ({ value, label: s.label })),
];
export const typeOptions = [
  { value: "all", label: "All types" },
  { value: "kind", label: "Kinds" },
  { value: "property", label: "Properties" },
  { value: "link", label: "Link types" },
  { value: "metric", label: "Metrics" },
];

export function Pagination({ page, total }: { page: number; total: number }) {
  const { update } = useTwinParams();
  const pages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  return (
    <nav
      aria-label="Pagination"
      className="flex flex-wrap items-center justify-between gap-2 text-detail pt-3"
    >
      <span style={{ color: "var(--ink-dim)" }}>
        {total === 0
          ? "No results"
          : `${(page - 1) * PAGE_SIZE + 1} to ${Math.min(page * PAGE_SIZE, total)} of ${total}`}
      </span>
      <div className="flex items-center gap-2">
        <button
          className="btn btn-sm"
          disabled={page <= 1}
          onClick={() => update({ page: String(page - 1) })}
        >
          Previous
        </button>
        <span>
          {page} / {pages}
        </span>
        <button
          className="btn btn-sm"
          disabled={page >= pages}
          onClick={() => update({ page: String(page + 1) })}
        >
          Next
        </button>
      </div>
    </nav>
  );
}
