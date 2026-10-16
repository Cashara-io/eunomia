import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createConnector, deleteConnector, deleteConnectorData, listConnectors, moveConnectorData, repointConnectors, testConnector, updateConnector } from "@/lib/gen";
import type { Connector, ConnectorKind, ConnectorUpdate, MoveReport } from "@/lib/types";

/** A connection id, or a kind for that kind's first connection (what the API accepts as `{connector}`). */
export type ConnectorRef = string;
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
    mutationFn: ({ connector, body }: { connector: ConnectorRef; body: ConnectorUpdate }) =>
      call(updateConnector({ path: { connector }, body })) as Promise<Connector>,
    // Enabling or disabling a connector changes which sources are connected.
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: connectorKeys.all }), qc.invalidateQueries({ queryKey: sourceKeys.all })]),
  });
}

export const useTestConnector = () => useMutation({ mutationFn: (connector: ConnectorRef) => call(testConnector({ path: { connector } })) });

/** "Add another": a further, empty connection of `kind`, named `label`. */
export function useCreateConnector() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { kind: ConnectorKind; label: string }) => call(createConnector({ body })) as Promise<Connector>,
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: connectorKeys.all }), qc.invalidateQueries({ queryKey: sourceKeys.all })]),
  });
}

/** Removes one connection and everything synced through it. */
export function useDeleteConnector() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (connector: ConnectorRef) => call(deleteConnector({ path: { connector } })),
    onSuccess: () => qc.invalidateQueries(),
  });
}

/** Deletes everything synced from one connector. Records, search, the graph and counts all change, so refetch them all. */
export function useDeleteConnectorData() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (connector: ConnectorRef) => call(deleteConnectorData({ path: { connector } })),
    onSuccess: () => qc.invalidateQueries(),
  });
}

/** "Move existing data": re-files what a connector already synced by its current routing. Records, search and the graph change. */
export function useMoveConnectorData() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (connector: ConnectorRef) => call(moveConnectorData({ path: { connector } })) as Promise<MoveReport>,
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
