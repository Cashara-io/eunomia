import { keepPreviousData, useMutation, useQueries, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import {
  addMemory,
  addRelation,
  createEntity,
  deleteEntity,
  deleteMemory,
  getEntity,
  getEntityGraph,
  getCloudSimilarity,
  getVectorCloud,
  listEntities,
  mergeEntities,
  updateEntity,
  updateMemory,
} from "@/lib/gen";
import type { Cloud, CloudSimilarity, EntityDetail, EntityGraph, EntityKind, EntityMemory, EntityRelation, EntitySummary } from "@/lib/types";
import { call } from "./client";
import { invoke } from "./tools";
import { livePoll } from "./liveStatus";

export const entityKeys = {
  all: ["entities"] as const,
  list: (kind?: EntityKind, vaultId?: string, limit?: number) => [...entityKeys.all, "list", kind ?? null, vaultId ?? null, limit ?? null] as const,
  detail: (id: string) => [...entityKeys.all, "detail", id] as const,
  graph: (kinds: EntityKind[] | undefined, vaultId: string | undefined) => [...entityKeys.all, "graph", kinds ?? null, vaultId ?? null] as const,
  cloud: (vaultIds: string[]) => [...entityKeys.all, "cloud", vaultIds] as const,
  similarity: (vaultIds: string[], ids: string[]) => [...entityKeys.all, "similarity", vaultIds, ids] as const,
  live: () => [...entityKeys.all, "live"] as const,
};

export type EntityList = { results: EntitySummary[]; total: number; has_more: boolean };

export const entityListQuery = (kind?: EntityKind, vaultId?: string, limit?: number) => ({
  queryKey: entityKeys.list(kind, vaultId, limit),
  queryFn: () => call(listEntities({ query: { kind, vault_id: vaultId, limit } })) as Promise<EntityList>,
  refetchInterval: livePoll,
});

/**
 * Details (memories, relations) for each of `ids`, for the List view's memory count, latest memory and
 * last update. The list endpoint carries none of these, so this is one detail read per row.
 */
export const useEntityDetails = (ids: string[]) =>
  useQueries({
    queries: ids.map((id) => ({ ...entityDetailQuery(id), staleTime: 30_000 })),
    combine: (results) => new Map(ids.map((id, i) => [id, results[i]?.data])),
  });
export const entityDetailQuery = (id: string) => ({
  queryKey: entityKeys.detail(id),
  queryFn: () => call(getEntity({ path: { entity_id: id } })) as Promise<EntityDetail>,
  staleTime: 0,
});

export const useEntityGraph = (opts: { kinds?: EntityKind[]; vaultId?: string; enabled?: boolean }) =>
  useQuery({
    enabled: opts.enabled ?? true,
    queryKey: entityKeys.graph(opts.kinds, opts.vaultId),
    queryFn: () => call(getEntityGraph({ query: { kinds: opts.kinds, vault_id: opts.vaultId } })) as Promise<EntityGraph>,
    // Switching vault keeps the old graph on screen until the new one lands.
    placeholderData: keepPreviousData,
    refetchInterval: livePoll,
  });

/** How many people and topics each of `vaultIds` holds (one `limit=1` list per space, reading only `total`). */
export const useEntityCounts = (vaultIds: string[], enabled = true) =>
  useQueries({
    queries: vaultIds.map((vaultId) => ({
      queryKey: [...entityKeys.all, "count", vaultId] as const,
      queryFn: async () => ((await call(listEntities({ query: { vault_id: vaultId, limit: 1 } }))) as EntityList).total,
      enabled,
      staleTime: 60_000,
    })),
    combine: (results) => ({
      pending: results.some((r) => r.isPending && r.fetchStatus !== "idle"),
      counts: new Map(vaultIds.map((id, i) => [id, results[i]?.data ?? 0])),
    }),
  });

/**
 * The memory space an entity lives in. The detail endpoint doesn't carry it, so: the detail's name,
 * then a name search across every space the caller can read, matched back by id. `null` when the
 * entity isn't readable or can't be found.
 */
export async function findEntityVault(qc: QueryClient, id: string): Promise<string | null> {
  const detail = await qc.fetchQuery(entityDetailQuery(id)).catch(() => null);
  if (!detail) return null;
  const res = await invoke<{ results: (EntitySummary & { vault?: string })[] } | { error: string }>("entities_search", {
    query: detail.name,
    limit: 50,
  }).catch(() => null);
  const hit = res && "results" in res ? res.results.find((r) => r.id === id) : undefined;
  return hit?.vault ?? (detail.memory[0] as { vault?: string } | undefined)?.vault ?? null;
}

export const useEntityCloud = (vaultIds: string[]) =>
  useQuery({
    queryKey: entityKeys.cloud(vaultIds),
    queryFn: () => call(getVectorCloud({ query: { vault_ids: vaultIds.join(",") } })) as Promise<Cloud>,
    // Layering another vault keeps the old points until the new cloud lands.
    placeholderData: keepPreviousData,
    enabled: vaultIds.length > 0,
    refetchInterval: livePoll,
  });

/** Cosine / distance / correlation between cloud points on their full vectors, computed server-side. */
export const useCloudSimilarity = (vaultIds: string[], ids: string[]) =>
  useQuery({
    queryKey: entityKeys.similarity(vaultIds, ids),
    queryFn: () => call(getCloudSimilarity({ body: { vault_ids: vaultIds, ids } })) as Promise<CloudSimilarity>,
    enabled: vaultIds.length > 0 && ids.length >= 2,
    placeholderData: keepPreviousData,
  });

// Memories live on entities, the graph and the cloud both draw them: any write refreshes the whole entity tree.
const useEntityMutation = <V, R>(fn: (v: V) => Promise<R>) => {
  const qc = useQueryClient();
  return useMutation({ mutationFn: fn, onSuccess: () => qc.invalidateQueries({ queryKey: entityKeys.all }) });
};

export const useCreateEntity = () =>
  useEntityMutation((body: { kind: EntityKind; name: string; aliases?: string[]; vault_id?: string }) =>
    call(createEntity({ body })) as Promise<EntitySummary>,
  );
export const useUpdateEntity = () =>
  useEntityMutation(({ id, ...body }: { id: string } & Partial<{ name: string; aliases: string[]; summary: string }>) =>
    call(updateEntity({ path: { entity_id: id }, body })) as Promise<EntitySummary>,
  );
export const useDeleteEntity = () => useEntityMutation((id: string) => call(deleteEntity({ path: { entity_id: id } })));
export const useAddMemory = () =>
  useEntityMutation(({ id, text }: { id: string; text: string }) =>
    call(addMemory({ path: { entity_id: id }, body: { text } })) as Promise<EntityMemory>,
  );
export const useDeleteMemory = () => useEntityMutation((memoryId: string) => call(deleteMemory({ path: { memory_id: memoryId } })));
export const useAddRelation = () =>
  useEntityMutation(({ id, to_id, label }: { id: string; to_id: string; label: string }) =>
    call(addRelation({ path: { entity_id: id }, body: { to_id, label } })) as Promise<EntityRelation>,
  );
export const useMergeEntities = () =>
  useEntityMutation(({ winnerId, loserId }: { winnerId: string; loserId: string }) =>
    call(mergeEntities({ path: { entity_id: winnerId }, body: { loser_id: loserId } })) as Promise<EntitySummary>,
  );

/** Edit a memory's text (the `memory_update` tool's REST twin). */
export const useUpdateMemory = () =>
  useEntityMutation(({ memoryId, text }: { memoryId: string; text: string }) =>
    call(updateMemory({ path: { memory_id: memoryId }, body: { text } })) as Promise<EntityMemory>,
  );

/** What `consolidate_observations` reports. */
export type ConsolidateResult = { consolidated?: string[]; skipped?: string[]; errors?: string[]; note?: string; error?: string };

/** Fold an entity's new facts into its summary observation (the `consolidate_observations` tool). Needs a model. */
export const useConsolidateObservations = () =>
  useEntityMutation((subjectId: string) => invoke<ConsolidateResult>("consolidate_observations", { subject_id: subjectId }));
