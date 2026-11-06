import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createSecretRef,
  deleteSecretProvider,
  deleteSecretRef,
  getSecretAccess,
  getSecretCounts,
  getSecretsHelper,
  listSecretKinds,
  listSecretProviders,
  listSecretRefs,
  listSecretResolutions,
  putSecretProvider,
  requestSecretsHelper,
  testSecretProvider,
  updateSecretRef,
} from "@/lib/gen";
import type { AccessOut, CheckOut, HelperStatus, Kind, LogPage, ProviderBody, ProviderOut, RefBody, RefCount, RefOut } from "@/lib/gen";
import { call } from "./client";

/** The credential broker's settings (docs/password-managers.md): password managers, references, the resolution log. */
export const secretKeys = {
  all: ["secrets"] as const,
  providers: () => [...secretKeys.all, "providers"] as const,
  refs: () => [...secretKeys.all, "refs"] as const,
  log: () => [...secretKeys.all, "log"] as const,
  counts: () => [...secretKeys.all, "counts"] as const,
  kinds: () => [...secretKeys.all, "kinds"] as const,
  access: () => [...secretKeys.all, "access"] as const,
  helper: () => [...secretKeys.all, "helper"] as const,
};

// 403 (not an owner) is an answer, not a failure worth retrying
const noRetry = { retry: false } as const;

/** The provider kinds with their stability (Bitwarden Secrets Manager is a preview). Static per release. */
export const useSecretKinds = () => useQuery({ queryKey: secretKeys.kinds(), queryFn: () => call(listSecretKinds()) as Promise<Kind[]>, staleTime: Infinity, ...noRetry });

/** What the signed-in user may do here: set up providers (org owner or admin), manage references (and for which vaults), switch the helper. */
export const useSecretAccess = () => useQuery({ queryKey: secretKeys.access(), queryFn: () => call(getSecretAccess()) as Promise<AccessOut>, ...noRetry });

/** The secrets helper's status. Quick while it is starting or a request waits for the updater, lazy otherwise. */
export const useSecretsHelper = () =>
  useQuery({
    queryKey: secretKeys.helper(),
    queryFn: () => call(getSecretsHelper()) as Promise<HelperStatus>,
    refetchInterval: (q) => (q.state.data?.state === "starting" || q.state.data?.pending ? 3000 : 15000),
  });

/** Only drops a request for the updater (instance admins); the status query shows the outcome. */
export function useRequestSecretsHelper() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (enabled: boolean) => call(requestSecretsHelper({ body: { enabled } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretKeys.helper() }),
  });
}

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
