import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createExtension, deleteExtension, listExtensions, rotateExtensionSecret, updateExtension } from "@/lib/gen";
import type { CreateBody, ExtensionOut, PatchBody } from "@/lib/gen";
import { call } from "./client";

export const extensionKeys = {
  all: ["extensions"] as const,
  list: () => [...extensionKeys.all, "list"] as const,
};

/** Inbox counts move as deliveries arrive and scripts ack them, so the list polls. */
export const useExtensions = () =>
  useQuery({ queryKey: extensionKeys.list(), queryFn: () => call(listExtensions()) as Promise<ExtensionOut[]>, refetchInterval: 5_000, staleTime: 2_000 });

export function useCreateExtension() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateBody) => call(createExtension({ body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: extensionKeys.all }),
  });
}

export function useUpdateExtension() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: PatchBody }) => call(updateExtension({ path: { id }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: extensionKeys.all }),
  });
}

export function useRotateExtensionSecret() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => call(rotateExtensionSecret({ path: { id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: extensionKeys.all }),
  });
}

export function useDeleteExtension() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => call(deleteExtension({ path: { id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: extensionKeys.all }),
  });
}
