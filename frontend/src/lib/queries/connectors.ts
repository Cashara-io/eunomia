import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { deleteConnectorData, listConnectors, moveConnectorData, repointConnectors, testConnector, updateConnector } from "@/lib/gen";
import type { Connector, ConnectorKind, ConnectorUpdate, MoveReport } from "@/lib/types";
import { call } from "./client";
import { sourceKeys } from "./sources";

export const connectorKeys = {
  all: ["connectors"] as const,
  list: () => [...connectorKeys.all, "list"] as const,
};

export const useConnectors = () =>
  useQuery({ queryKey: connectorKeys.list(), queryFn: () => call(listConnectors()) as Promise<Connector[]> });

export function useUpdateConnector() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ kind, body }: { kind: ConnectorKind; body: ConnectorUpdate }) =>
      call(updateConnector({ path: { kind }, body })) as Promise<Connector>,
    // Enabling or disabling a connector changes which sources are connected.
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: connectorKeys.all }), qc.invalidateQueries({ queryKey: sourceKeys.all })]),
  });
}

export const useTestConnector = () => useMutation({ mutationFn: (kind: ConnectorKind) => call(testConnector({ path: { kind } })) });

/** Deletes everything synced from one connector. Records, search, the graph and counts all change, so refetch them all. */
export function useDeleteConnectorData() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (kind: ConnectorKind) => call(deleteConnectorData({ path: { kind } })),
    onSuccess: () => qc.invalidateQueries(),
  });
}

/** "Move existing data": re-files what a connector already synced by its current routing. Records, search and the graph change. */
export function useMoveConnectorData() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (kind: ConnectorKind) => call(moveConnectorData({ path: { kind } })) as Promise<MoveReport>,
    onSuccess: () => qc.invalidateQueries(),
  });
}

/** After a vault merge: connectors filing into `from` file into `to` instead. */
export function useRepointConnectors() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { from: string[]; to: string }) => call(repointConnectors({ body })) as Promise<{ connectors: number }>,
    onSuccess: () => qc.invalidateQueries({ queryKey: connectorKeys.all }),
  });
}
