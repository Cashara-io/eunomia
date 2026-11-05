// Sharing a memory space with another Eunomia (backend `routers/peers.rs`, docs/peer-vaults.md).
// A small hand-written client: these routes are new, so they are typed here rather than in lib/gen.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { API_URL, ApiError, traceparent } from "@/lib/api";
import { vaultKeys } from "./vaults";
import { entityKeys } from "./entities";

export type PeerStatus = "up_to_date" | "syncing" | "unreachable" | "integrity_problem" | "revoked";
export type PeerRole = "edit" | "read";

export type PeerLink = {
  id: string;
  /** host: this Eunomia has the original and shares it out; guest: this space is a copy of another's. */
  direction: "host" | "guest";
  role: PeerRole;
  peer_name: string;
  peer_fingerprint: string;
  status: PeerStatus;
  last_sync_at: string | null;
  address: string | null;
  digests_match: boolean | null;
  integrity_failures: number;
  last_error: string | null;
  last_report: { received?: Record<string, number>; sent?: Record<string, number>; documents_not_shared?: number } | null;
};

export type PendingCode = { id: string; role: PeerRole; created_at: string | null; expires_at: string | null };

export type PeersOut = { vault: string; fingerprint: string; links: PeerLink[]; codes: PendingCode[]; addresses: string[] };

export type ShareCode = { id: string; code: string; role: PeerRole; expires_at: string; addresses: string[]; fingerprint: string };

export type JoinOut = { vault: { id: string; name: string }; link: PeerLink };

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${API_URL}${path}`, {
    method,
    credentials: "include",
    headers: { traceparent: traceparent(), ...(body !== undefined ? { "content-type": "application/json" } : {}) },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    let detail = `${res.status} ${path}`;
    let code = `http.${res.status}`;
    let traceId = res.headers.get("x-trace-id") ?? undefined;
    try {
      const b = await res.json();
      if (typeof b?.detail === "string") detail = b.detail;
      if (typeof b?.code === "string") code = b.code;
      if (typeof b?.trace_id === "string") traceId = b.trace_id;
    } catch {
      // not JSON: keep the status line
    }
    throw new ApiError(res.status, code, detail, traceId);
  }
  return (await res.json()) as T;
}

const enc = encodeURIComponent;

export const peerKeys = {
  all: ["peers"] as const,
  vault: (vaultId: string) => [...peerKeys.all, "vault", vaultId] as const,
};

/** Who a space is shared with and how syncing is going: refreshed every 10 s while the page is visible. */
export const useVaultPeers = (vaultId: string, enabled = true) =>
  useQuery({
    queryKey: peerKeys.vault(vaultId),
    queryFn: () => request<PeersOut>("GET", `/api/vaults/${enc(vaultId)}/peers`),
    refetchInterval: 10_000,
    enabled,
  });

export function useCreateShareCode(vaultId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (role: PeerRole) => request<ShareCode>("POST", `/api/vaults/${enc(vaultId)}/share-codes`, { role }),
    onSuccess: () => qc.invalidateQueries({ queryKey: peerKeys.vault(vaultId) }),
  });
}

export function useDeleteShareCode(vaultId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (codeId: string) => request<{ deleted: boolean }>("DELETE", `/api/vaults/${enc(vaultId)}/share-codes/${enc(codeId)}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: peerKeys.vault(vaultId) }),
  });
}

export function useSyncPeer(vaultId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (linkId: string) => request<PeerLink>("POST", `/api/peers/links/${enc(linkId)}/sync`),
    onSettled: () => Promise.all([qc.invalidateQueries({ queryKey: peerKeys.vault(vaultId) }), qc.invalidateQueries({ queryKey: entityKeys.all })]),
  });
}

export function useStopPeer(vaultId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (linkId: string) => request<{ deleted: boolean }>("DELETE", `/api/peers/links/${enc(linkId)}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: peerKeys.vault(vaultId) }),
  });
}

export function useJoinShared() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (code: string) => request<JoinOut>("POST", "/api/peers/join", { code }),
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: vaultKeys.all }), qc.invalidateQueries({ queryKey: entityKeys.all }), qc.invalidateQueries({ queryKey: peerKeys.all })]),
  });
}
