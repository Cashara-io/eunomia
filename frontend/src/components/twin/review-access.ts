import type { QueryClient } from "@tanstack/react-query";
import type { Vault } from "@/lib/types";
import { vaultKeys } from "@/lib/queries/vaults";

export const ownerOnly =
  "Only an owner of this space can approve, reject or merge a definition.";

export function isReviewOwner(cache: QueryClient, vault: string) {
  return cache.getQueryData<Vault[]>(vaultKeys.list())
    ?.find(item => item.id === vault)?.role === "owner";
}
