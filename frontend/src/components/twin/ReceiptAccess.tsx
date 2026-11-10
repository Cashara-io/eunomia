"use client";
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listMembers } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import { useVaults } from "@/lib/queries/vaults";
import { ApiError } from "@/lib/api";
import ErrorLine from "@/components/ErrorLine";
const ConfirmedSpace = createContext("");
const protectedKeys = ["object-profile", "objects", "object-kinds", "ontology-activity"];
export default function ReceiptAccess({ space, children }: { space: string; children: ReactNode }) {
  const confirmed = useContext(ConfirmedSpace);
  return confirmed === space ? children : <CheckAccess space={space}>{children}</CheckAccess>;
}
function CheckAccess({ space, children }: { space: string; children: ReactNode }) {
  const [blocked, setBlocked] = useState(false);
  const cache = useQueryClient();
  const vaults = useVaults();
  const membership = useQuery({
    queryKey: ["receipt-access", space],
    queryFn: ({ signal }) => call(listMembers({ path: { vault_id: space }, signal })),
    staleTime: 0, refetchOnWindowFocus: true, retry: false,
  });
  const denied = membership.error instanceof ApiError && ["vault.forbidden", "vault.not_found"].includes(membership.error.code);
  const missingVault = vaults.isSuccess && !vaults.isFetching && !vaults.data.some(v => v.id === space);
  useEffect(() => {
    const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => protectedKeys.includes(String(q.queryKey[0])) && q.queryKey[1] === space };
    if (blocked || denied || missingVault || membership.isError) { void cache.cancelQueries(scoped); cache.removeQueries(scoped); }
  }, [cache, blocked, denied, missingVault, membership.isError, space]);
  useEffect(() => {
    const seen = new Set<unknown>();
    return cache.getQueryCache().subscribe(event => {
      if (event.type !== "updated" || event.query.queryKey[1] !== space || !protectedKeys.includes(String(event.query.queryKey[0]))) return;
      const error = event.query.state.error ?? event.query.state.fetchFailureReason;
      if (!(error instanceof ApiError) || seen.has(error)) return;
      seen.add(error);
      if (error.code === "vault.forbidden" || error.status === 403) setBlocked(true);
      else if (error.code === "entity.not_found") void cache.invalidateQueries({ queryKey: ["receipt-access", space] });
    });
  }, [cache, space]);
  if (blocked || denied || missingVault) return <ErrorLine>This information is unavailable to you in this space. Ask a space owner for access.</ErrorLine>;
  if (membership.isError || vaults.isError) return <ErrorLine action={<button className="btn btn-sm" onClick={() => { void membership.refetch(); void vaults.refetch(); }}>Retry access check</button>}>Could not confirm space access. Protected receipts are hidden.</ErrorLine>;
  if (membership.isPending || membership.isFetching || vaults.isFetching) return <p role="status">Checking space access…</p>;
  return <ConfirmedSpace.Provider value={space}>{children}</ConfirmedSpace.Provider>;
}
