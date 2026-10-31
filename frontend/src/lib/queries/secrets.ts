import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createSecretRef,
  deleteSecretProvider,
  deleteSecretRef,
  getSecretCounts,
  listSecretProviders,
  listSecretRefs,
  listSecretResolutions,
  putSecretProvider,
  testSecretProvider,
  updateSecretRef,
} from "@/lib/gen";
import type { CheckOut, LogPage, ProviderBody, ProviderOut, RefBody, RefCount, RefOut } from "@/lib/gen";
import { call } from "./client";

/** The credential broker's settings (docs/password-managers.md): password managers, references, the resolution log. */
export const secretKeys = {
  all: ["secrets"] as const,
  providers: () => [...secretKeys.all, "providers"] as const,
  refs: () => [...secretKeys.all, "refs"] as const,
  log: () => [...secretKeys.all, "log"] as const,
  counts: () => [...secretKeys.all, "counts"] as const,
};

// 403 (not an owner) is an answer, not a failure worth retrying
const noRetry = { retry: false } as const;

export const useSecretProviders = () => useQuery({ queryKey: secretKeys.providers(), queryFn: () => call(listSecretProviders()) as Promise<ProviderOut[]>, ...noRetry });

export const useSecretRefs = () => useQuery({ queryKey: secretKeys.refs(), queryFn: () => call(listSecretRefs()) as Promise<RefOut[]>, ...noRetry });

export const useSecretLog = () =>
  useQuery({ queryKey: secretKeys.log(), queryFn: () => call(listSecretResolutions({ query: { limit: 50 } })) as Promise<LogPage>, refetchInterval: 10_000, ...noRetry });

export const useSecretCounts = () =>
  useQuery({ queryKey: secretKeys.counts(), queryFn: () => call(getSecretCounts({ query: { hours: 168 } })) as Promise<RefCount[]>, refetchInterval: 10_000, ...noRetry });

export function usePutSecretProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ kind, body }: { kind: string; body: ProviderBody }) => call(putSecretProvider({ path: { kind }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.providers() }),
  });
}

export function useDeleteSecretProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (kind: string) => call(deleteSecretProvider({ path: { kind } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.providers() }),
  });
}

export const useTestSecretProvider = () => useMutation({ mutationFn: (kind: string) => call(testSecretProvider({ path: { kind } })) as Promise<CheckOut> });

export function useCreateSecretRef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: RefBody) => call(createSecretRef({ body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.refs() }),
  });
}

export function useUpdateSecretRef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: RefBody }) => call(updateSecretRef({ path: { id }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.refs() }),
  });
}

export function useDeleteSecretRef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => call(deleteSecretRef({ path: { id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.refs() }),
  });
}
