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

/** A memory whose text matches a palette search, with the entity it is about and its space. */
export type MemoryHit = { id: string; text: string; entity: string; entityName: string; entityKind: string; vault: string; vaultName?: string };
type RecallHit = { id: string; kind: string; text: string; vault: string; vault_name: string };
type EntityDetail = { id: string; kind: string; name: string; memory?: { id: string; text: string; vault: string }[] };

/** Every word of `query` (2+ letters), lowercased: a memory matches when its text holds one of them. */
export const queryWords = (query: string) =>
  query.toLowerCase().split(/\s+/).filter((w) => w.length >= 2);
const mentions = (text: string, words: string[]) => {
  const t = text.toLowerCase();
  return words.some((w) => t.includes(w));
};

/** Entities a memory search resolves memories against: the fact matches first (names that do not hold
 * the query), then name matches, at most this many. */
const RESOLVE_ENTITIES = 4;
/** Memories shown in the palette. */
const PALETTE_MEMORIES = 6;

/** Command palette: entity, memory and record matches for a debounced query. Failures read as no matches. */
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
  // Memory text. `recall` ranks the memories; it does not say which entity a memory is about, so the
  // memories of the entities `entities_search` found (it matches fact text too) name each one. When
  // recall fails (no embedding model, say) those entities' matching memories are the result.
  const found = entities.isPlaceholderData ? undefined : entities.data;
  const words = queryWords(query);
  const resolveIds = (found ?? [])
    .map((e) => ({ id: e.id, byName: mentions(e.name, words) }))
    .sort((a, b) => Number(a.byName) - Number(b.byName))
    .slice(0, RESOLVE_ENTITIES)
    .map((e) => e.id);
  const memories = useQuery({
    queryKey: [...toolKeys.palette(query), "memories", resolveIds],
    queryFn: async (): Promise<MemoryHit[]> => {
      const [recall, ...details] = await Promise.all([
        invoke<{ results: RecallHit[] } | { error: string }>("recall", { query, limit: PALETTE_MEMORIES }).catch(() => ({ error: "" })),
        ...resolveIds.map((id) => invoke<EntityDetail | { error: string }>("entities_get", { id }).catch(() => ({ error: "" }))),
      ]);
      const about = new Map<string, MemoryHit>();
      for (const d of details) {
        if ("error" in d) continue;
        for (const m of d.memory ?? []) {
          if (mentions(m.text, words)) {
            about.set(m.id, { id: m.id, text: m.text, entity: d.id, entityName: d.name, entityKind: d.kind, vault: m.vault });
          }
        }
      }
      if ("error" in recall) return [...about.values()].slice(0, PALETTE_MEMORIES);
      const ranked = recall.results
        .filter((r) => r.kind === "memory" && about.has(r.id))
        .map((r) => ({ ...about.get(r.id)!, vaultName: r.vault_name || undefined }));
      // a match recall ranked below its limit still shows, after the ranked ones
      const seen = new Set(ranked.map((m) => m.id));
      return [...ranked, ...[...about.values()].filter((m) => !seen.has(m.id))].slice(0, PALETTE_MEMORIES);
    },
    enabled: enabled && found !== undefined && words.length > 0,
    placeholderData: keepPreviousData,
  });
  return { entities: entities.data ?? [], records: records.data ?? [], memories: memories.data ?? [] };
};

export const useDocs = () =>
  useQuery({ queryKey: toolKeys.docs(), queryFn: () => invoke<{ docs: { topic: string; title: string }[] }>("docs") });

export const useDoc = (topic: string) =>
  useQuery({
    queryKey: toolKeys.docs(topic),
    queryFn: () => invoke<{ topic: string; title: string; markdown: string }>("docs", { topic }),
    placeholderData: keepPreviousData,
  });
