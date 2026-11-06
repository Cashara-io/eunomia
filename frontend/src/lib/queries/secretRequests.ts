import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { declineSecretRequest, deleteHeldSecret, fillSecretRequest, getSecretRequest, listHeldSecrets, listSecretActivity, listSecretRequests, replaceHeldSecret } from "@/lib/gen";
import type { ActivityPage, FillBody, FillOut, HeldOut, RequestOut } from "@/lib/gen";
import { call } from "./client";

/** Secret requests and the secrets Eunomia holds (docs/secret-requests.md). Nothing here ever carries a value back. */
export const secretRequestKeys = {
  all: ["secret-requests"] as const,
  pending: () => [...secretRequestKeys.all, "pending"] as const,
  list: () => [...secretRequestKeys.all, "list"] as const,
  one: (id: string) => [...secretRequestKeys.all, "one", id] as const,
  held: () => [...secretRequestKeys.all, "held"] as const,
  activity: () => [...secretRequestKeys.all, "activity"] as const,
};

const noRetry = { retry: false } as const;

/** Pending requests, polled while the tab is visible: what the pop-up shows. */
export const usePendingSecretRequests = (enabled = true) =>
  useQuery({
    queryKey: secretRequestKeys.pending(),
    queryFn: () => call(listSecretRequests({ query: { status: "pending" } })) as Promise<RequestOut[]>,
    refetchInterval: 5_000,
    refetchIntervalInBackground: false,
    enabled,
    ...noRetry,
  });

export const useSecretRequests = () => useQuery({ queryKey: secretRequestKeys.list(), queryFn: () => call(listSecretRequests()) as Promise<RequestOut[]>, refetchInterval: 10_000, ...noRetry });

export const useSecretRequest = (id: string) => useQuery({ queryKey: secretRequestKeys.one(id), queryFn: () => call(getSecretRequest({ path: { id } })) as Promise<RequestOut>, ...noRetry });

export const useHeldSecrets = () => useQuery({ queryKey: secretRequestKeys.held(), queryFn: () => call(listHeldSecrets()) as Promise<HeldOut[]>, ...noRetry });

export const useSecretActivity = () =>
  useQuery({ queryKey: secretRequestKeys.activity(), queryFn: () => call(listSecretActivity({ query: { limit: 50 } })) as Promise<ActivityPage>, refetchInterval: 10_000, ...noRetry });

export function useFillSecretRequest() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: FillBody }) => call(fillSecretRequest({ path: { id }, body })) as Promise<FillOut>,
    onSuccess: () => qc.invalidateQueries({ queryKey: secretRequestKeys.all }),
  });
}

export function useDeclineSecretRequest() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => call(declineSecretRequest({ path: { id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretRequestKeys.all }),
  });
}

export function useReplaceHeldSecret() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, value }: { id: string; value: string }) => call(replaceHeldSecret({ path: { id }, body: { value } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretRequestKeys.all }),
  });
}

export function useDeleteHeldSecret() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => call(deleteHeldSecret({ path: { id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: secretRequestKeys.all }),
  });
}
