import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { invokeTool } from "@/lib/gen";
import type { EntitySummary, ToolHit, ToolRecord } from "@/lib/types";
import { ApiError } from "@/lib/api";
import { call } from "./client";

export const toolKeys = {
  all: ["tools"] as const,
  list: (source: string, offset = 0) => [...toolKeys.all, "list", source, offset] as const,
  search: (source: string, query: string, offset = 0) => [...toolKeys.all, "search", source, query, offset] as const,
  record: (id: string) => [...toolKeys.all, "record", id] as const,
  palette: (query: string) => [...toolKeys.all, "palette", query] as const,
  docs: (topic?: string) => [...toolKeys.all, "docs", topic ?? "index"] as const,
};

// Tools answer HTTP 200 with `{ error }` for a bad call, so a failure is a value here, not a throw.
type Hits = { results: ToolHit[] } | { error: string };
export const invoke = <T>(name: string, body: Record<string, unknown> = {}) => call(invokeTool({ path: { name }, body })) as Promise<T>;
const hits = (res: Hits) => ("results" in res ? res.results : []);

/** Records shown per page on a connector's record table. */
export const RECORDS_PAGE = 50;
/** One page of a source's records; `offset` echoes the request, `total` is set for a plain list only. */
export type RecordPage = { results: ToolHit[]; offset: number; has_more: boolean; total?: number };
type Paged = { results: ToolHit[]; has_more: boolean; total?: number } | { error: string };
const page = (res: Paged, offset: number): RecordPage =>
  "results" in res ? { ...res, offset } : { results: [], offset, has_more: false, total: 0 };

/** The backend's search window (`MAX_WINDOW`): a search pages through at most this many matches. */
export const SEARCH_WINDOW = 1000;

// The previous page of the same source stays on screen while the next one loads.
export const useSourceRecords = (source: string, offset: number) =>
  useQuery({
    queryKey: toolKeys.list(source, offset),
    queryFn: async () =>
      page(await invoke<Paged>("list", { filters: { source }, sort: "-occurred_at", limit: RECORDS_PAGE, offset }), offset),
    placeholderData: (prev, prevQuery) => (prevQuery?.queryKey[2] === source ? prev : undefined),
  });

// `query` is "" for no search; the previous results stay on screen while a new search loads.
// Keyword mode: its ranking does not shift between pages, so paging shows each match once.
export const useSourceSearch = (source: string, query: string, offset: number) =>
  useQuery({
    queryKey: toolKeys.search(source, query, offset),
    queryFn: async () =>
      page(await invoke<Paged>("search", { query, sources: [source], mode: "keyword", limit: RECORDS_PAGE, offset }), offset),
    enabled: query !== "",
    placeholderData: keepPreviousData,
  });

export const useRecord = (id: string, enabled: boolean) =>
  useQuery({
    queryKey: toolKeys.record(id),
    queryFn: async () => {
      const res = await invoke<ToolRecord | { error: string }>("get", { id });
      // A tool-level failure is a 200 with `{ error }`; surface it as a non-retried error.
      if ("error" in res) throw new ApiError(200, "tool.error", res.error);
      return res;
    },
    enabled,
  });

/** Command palette: entity and record matches for a debounced query. Failures read as no matches. */
export const usePaletteSearch = (query: string, limit: number, enabled: boolean) => {
  const entities = useQuery({
    queryKey: [...toolKeys.palette(query), "entities"],
    queryFn: async () => {
      const res = await invoke<{ results: EntitySummary[] } | { error: string }>("entities_search", { query, limit });
      return "results" in res ? res.results : [];
    },
    enabled,
    placeholderData: keepPreviousData,
  });
  const records = useQuery({
    queryKey: [...toolKeys.palette(query), "records"],
    queryFn: async () => hits(await invoke<Hits>("search", { query, limit })),
    enabled,
    placeholderData: keepPreviousData,
  });
  return { entities: entities.data ?? [], records: records.data ?? [] };
};

export const useDocs = () =>
  useQuery({ queryKey: toolKeys.docs(), queryFn: () => invoke<{ docs: { topic: string; title: string }[] }>("docs") });

export const useDoc = (topic: string) =>
  useQuery({
    queryKey: toolKeys.docs(topic),
    queryFn: () => invoke<{ topic: string; title: string; markdown: string }>("docs", { topic }),
    placeholderData: keepPreviousData,
  });
