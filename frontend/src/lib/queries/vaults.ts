import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  acceptInvitation,
  cloneVault,
  createVault,
  declineInvitation,
  deleteVault,
  inviteMember,
  leaveVault,
  listEntities,
  listInvitations,
  listMembers,
  listVaultPendingInvitations,
  listVaults,
  mergeVaults,
  removeMember,
  renameVault,
  setMemberRole,
  transferOwnership,
} from "@/lib/gen";
import type { Vault, VaultInvitation, VaultKind, VaultMember, VaultRole } from "@/lib/types";
import { call } from "./client";
import { livePoll } from "./liveStatus";
import { entityKeys } from "./entities";

export const vaultKeys = {
  all: ["vaults"] as const,
  list: () => [...vaultKeys.all, "list"] as const,
  members: (id: string) => [...vaultKeys.all, "members", id] as const,
  invitations: () => [...vaultKeys.all, "invitations"] as const,
  pending: (id: string) => [...vaultKeys.all, "pending", id] as const,
  counts: (id: string) => [...vaultKeys.all, "counts", id] as const,
};

/** An invitation to the caller. `invited_by` is the space's owner when it has exactly one, else absent. */
export type MyInvitation = VaultInvitation & { invited_by?: string | null };
/** An invitation into a space that the invitee hasn't answered yet, as its owners see it. */
export type PendingInvite = { email: string; role: VaultRole; created_at: string | null };

export const useVaults = () =>
  useQuery({
    queryKey: vaultKeys.list(),
    queryFn: async () => (await call(listVaults())).results as Vault[],
    refetchInterval: livePoll,
  });

export const useVaultMembers = (id: string, enabled = true) =>
  useQuery({
    queryKey: vaultKeys.members(id),
    queryFn: async () => (await call(listMembers({ path: { vault_id: id } }))).results as VaultMember[],
    // Not polled: after leaving or deleting a vault the inspector can still be mounted for a moment, and a
    // poll would hit a vault the caller no longer belongs to (403). Mutations and Refresh keep it current.
    enabled,
  });

export const useInvitations = () =>
  useQuery({
    queryKey: vaultKeys.invitations(),
    queryFn: async () => (await call(listInvitations())).results as MyInvitation[],
    refetchInterval: livePoll,
  });

/** How many invitations are waiting for the caller (0 while loading or on error), for a badge. */
export const usePendingInvitationCount = () => useInvitations().data?.length ?? 0;

/** Owners only: invitations into `id` nobody has answered yet. Withdraw one with `useRemoveMember`. */
export const usePendingInvites = (id: string, enabled = true) =>
  useQuery({
    queryKey: vaultKeys.pending(id),
    queryFn: async () => (await call(listVaultPendingInvitations({ path: { vault_id: id } }))).results as PendingInvite[],
    enabled,
  });

/** People and topics in a space, from the entity list's totals (no rows fetched). */
export const useVaultCounts = (id: string, enabled = true) =>
  useQuery({
    queryKey: vaultKeys.counts(id),
    queryFn: async () => {
      const [all, people] = await Promise.all([
        call(listEntities({ query: { vault_id: id, limit: 0 } })),
        call(listEntities({ query: { vault_id: id, kind: "person", limit: 0 } })),
      ]);
      return { people: people.total, topics: Math.max(0, all.total - people.total) };
    },
    enabled,
  });

// Vault changes reshape what the entity graph and cloud can show (clone, merge, delete, accept), so those refresh too.
const useVaultMutation = <V, R>(fn: (v: V) => Promise<R>) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    // A space's counts are marked stale but not refetched here: after leaving or deleting a space its
    // detail can stay mounted for a moment, and an immediate refetch would hit a 403. They reload the
    // next time a space detail mounts.
    onSuccess: () =>
      Promise.all([
        qc.invalidateQueries({ queryKey: vaultKeys.all, predicate: (q) => !q.queryKey.includes("counts") }),
        qc.invalidateQueries({ queryKey: vaultKeys.all, predicate: (q) => q.queryKey.includes("counts"), refetchType: "none" }),
        qc.invalidateQueries({ queryKey: entityKeys.all }),
      ]),
  });
};

export const useCreateVault = () =>
  useVaultMutation(({ name, kind = "org" }: { name: string; kind?: VaultKind }) => call(createVault({ body: { name, kind } })));
export const useRenameVault = () =>
  useVaultMutation(({ id, name }: { id: string; name: string }) => call(renameVault({ path: { vault_id: id }, body: { name } })));
// Leaving or deleting ends your access: refresh everything except that vault's own members, which would now answer 403.
const useEndAccessMutation = (fn: (id: string) => Promise<unknown>) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: async (_d, id) => {
      // A list poll already in flight would land after this and put the vault back on screen, remounting
      // its inspector and asking for members the caller can no longer see.
      await qc.cancelQueries({ queryKey: vaultKeys.members(id) });
      await qc.cancelQueries({ queryKey: vaultKeys.list() });
      qc.removeQueries({ queryKey: vaultKeys.members(id) });
      await Promise.all([
        qc.invalidateQueries({ queryKey: vaultKeys.all, predicate: (q) => q.queryKey[1] !== "members" || q.queryKey[2] !== id }),
        qc.invalidateQueries({ queryKey: entityKeys.all }),
      ]);
    },
  });
};
export const useDeleteVault = () => useEndAccessMutation((id: string) => call(deleteVault({ path: { vault_id: id } })));
export const useInviteMember = () =>
  useVaultMutation(({ id, email, role = "member" }: { id: string; email: string; role?: VaultRole }) =>
    call(inviteMember({ path: { vault_id: id }, body: { email, role } })),
  );
export const useRemoveMember = () =>
  useVaultMutation(({ id, email }: { id: string; email: string }) => call(removeMember({ path: { vault_id: id, email } })));
export const useSetMemberRole = () =>
  useVaultMutation(({ id, email, role }: { id: string; email: string; role: VaultRole }) =>
    call(setMemberRole({ path: { vault_id: id, email }, body: { role } })),
  );
export const useTransferOwnership = () =>
  useVaultMutation(({ id, email }: { id: string; email: string }) => call(transferOwnership({ path: { vault_id: id }, body: { email } })));
export const useLeaveVault = () => useEndAccessMutation((id: string) => call(leaveVault({ path: { vault_id: id } })));
// Accepting does not refresh by itself: the page plays its join animation first, then calls `useRefreshVaults()`.
export const useAcceptInvitation = () => useMutation({ mutationFn: (id: string) => call(acceptInvitation({ path: { vault_id: id } })) });
export function useRefreshVaults() {
  const qc = useQueryClient();
  return () => Promise.all([qc.invalidateQueries({ queryKey: vaultKeys.all }), qc.invalidateQueries({ queryKey: entityKeys.all })]);
}
export const useDeclineInvitation = () => useVaultMutation((id: string) => call(declineInvitation({ path: { vault_id: id } })));
export const useCloneVault = () =>
  useVaultMutation(({ id, name, kind = "org" }: { id: string; name?: string; kind?: VaultKind }) =>
    call(cloneVault({ path: { vault_id: id }, body: { name, kind } })),
  );
// Copies both into a NEW vault; the originals are untouched.
export const useMergeVaults = () =>
  useVaultMutation(({ a, b, name }: { a: string; b: string; name?: string }) =>
    call(mergeVaults({ body: { vault_ids: [a, b], name: name || undefined } })),
  );
