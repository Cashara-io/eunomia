import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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
} from "@/lib/gen";
import type { Cloud, CloudSimilarity, EntityDetail, EntityGraph, EntityKind, EntityMemory, EntityRelation, EntitySummary } from "@/lib/types";
import { call } from "./client";

export const entityKeys = {
  all: ["entities"] as const,
  list: (kind?: EntityKind, vaultId?: string) => [...entityKeys.all, "list", kind ?? null, vaultId ?? null] as const,
  detail: (id: string) => [...entityKeys.all, "detail", id] as const,
  graph: (kinds: EntityKind[] | undefined, vaultId: string | undefined) => [...entityKeys.all, "graph", kinds ?? null, vaultId ?? null] as const,
  cloud: (vaultIds: string[]) => [...entityKeys.all, "cloud", vaultIds] as const,
  similarity: (vaultIds: string[], ids: string[]) => [...entityKeys.all, "similarity", vaultIds, ids] as const,
  live: () => [...entityKeys.all, "live"] as const,
};

export type EntityList = { results: EntitySummary[]; total: number; has_more: boolean };

export const entityListQuery = (kind?: EntityKind, vaultId?: string) => ({
  queryKey: entityKeys.list(kind, vaultId),
  queryFn: () => call(listEntities({ query: { kind, vault_id: vaultId } })) as Promise<EntityList>,
});
export const entityDetailQuery = (id: string) => ({
  queryKey: entityKeys.detail(id),
  queryFn: () => call(getEntity({ path: { entity_id: id } })) as Promise<EntityDetail>,
  staleTime: 0,
});

export const useEntityGraph = (opts: { kinds?: EntityKind[]; vaultId?: string }) =>
  useQuery({
    queryKey: entityKeys.graph(opts.kinds, opts.vaultId),
    queryFn: () => call(getEntityGraph({ query: { kinds: opts.kinds, vault_id: opts.vaultId } })) as Promise<EntityGraph>,
    // Switching vault keeps the old graph on screen until the new one lands.
    placeholderData: keepPreviousData,
  });

export const useEntityCloud = (vaultIds: string[]) =>
  useQuery({
    queryKey: entityKeys.cloud(vaultIds),
    queryFn: () => call(getVectorCloud({ query: { vault_ids: vaultIds.join(",") } })) as Promise<Cloud>,
    // Layering another vault keeps the old points until the new cloud lands.
    placeholderData: keepPreviousData,
    enabled: vaultIds.length > 0,
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
