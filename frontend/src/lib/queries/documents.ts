import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { deleteDocument, getDocument, importDocumentRows, listDocuments, profileDocument, reindexDocument } from "@/lib/gen";
import type { DocumentDetail, DocumentList, ImportOut, TableProfile } from "@/lib/gen";
import { call } from "./client";
import { livePoll } from "./liveStatus";
import { toolKeys } from "./tools";

export const documentKeys = {
  all: ["documents"] as const,
  list: () => [...documentKeys.all, "list"] as const,
  detail: (id: string) => [...documentKeys.all, "detail", id] as const,
  profile: (id: string) => [...documentKeys.all, "profile", id] as const,
  inVault: (vaultId: string) => [...documentKeys.all, "vault", vaultId] as const,
};

const indexing = (list?: DocumentList) => list?.results.some((d) => d.status === "indexing") ?? false;

/** The user's documents. Polls every 2s while one is still indexing, so its status and passage count arrive on their own; otherwise only while live updates are down. */
export const useDocuments = () =>
  useQuery({
    queryKey: documentKeys.list(),
    queryFn: () => call(listDocuments({ query: { limit: 200 } })),
    refetchInterval: (q) => (indexing(q.state.data) ? 2000 : livePoll()),
  });

export const useDocument = (id: string | null) =>
  useQuery({
    queryKey: documentKeys.detail(id ?? ""),
    queryFn: () => call(getDocument({ path: { document_id: id! } })) as Promise<DocumentDetail>,
    enabled: id !== null,
    refetchInterval: (q) => (q.state.data?.document.status === "indexing" ? 2000 : false),
  });

// A change to a document changes what search finds: refresh the palette's record hits too.
const useDocumentMutation = <V, R>(fn: (v: V) => Promise<R>) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: documentKeys.all }), qc.invalidateQueries({ queryKey: toolKeys.all })]),
  });
};

export const useDeleteDocument = () => useDocumentMutation((id: string) => call(deleteDocument({ path: { document_id: id } })));
export const useReindexDocument = () => useDocumentMutation((id: string) => call(reindexDocument({ path: { document_id: id } })));

/** Call after an upload finishes (uploads go through `uploadDocumentFile`, which reports progress). */
export const useRefreshDocuments = () => {
  const qc = useQueryClient();
  return () => Promise.all([qc.invalidateQueries({ queryKey: documentKeys.all }), qc.invalidateQueries({ queryKey: toolKeys.all })]);
};

/** One memory space's documents (Kinds and links offers its CSVs to auto-map). */
export const useVaultDocuments = (vaultId: string, enabled = true) =>
  useQuery({
    queryKey: documentKeys.inVault(vaultId),
    queryFn: () => call(listDocuments({ query: { vault_id: vaultId, limit: 200 } })),
    enabled,
  });

/** A CSV's column profile and whether its rows can be imported now (read from the stored file). */
export const useDocumentProfile = (id: string, enabled: boolean) =>
  useQuery({
    queryKey: documentKeys.profile(id),
    queryFn: () => call(profileDocument({ path: { document_id: id } })) as Promise<TableProfile>,
    enabled,
  });

/** Imports a CSV's rows; the entities, the ontology's value counts and the profile all change. */
export const useImportRows = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, kind, name_column }: { id: string; kind: string; name_column: string }) =>
      call(importDocumentRows({ path: { document_id: id }, body: { kind, name_column } })) as Promise<ImportOut>,
    onSuccess: () => qc.invalidateQueries(),
  });
};
