// A memory space's ontology: the kinds and link types it uses, and what its owners decided about them
// (backend `routers/ontology.rs`). Deciding works only from a browser session.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  decideVaultOntologyKind,
  decideVaultOntologyLink,
  getVaultOntology,
  suggestVaultOntology,
  type KindDecision,
  type LinkDecision,
  type OntologyOut,
} from "@/lib/gen";
import { call } from "./client";
import { entityKeys } from "./entities";

export type { KindItem, LinkItem, OntologyOut, SuggestOut, ViewStatus } from "@/lib/gen";

export const ontologyKeys = {
  all: ["ontology"] as const,
  vault: (id: string) => [...ontologyKeys.all, id] as const,
};

export const useOntology = (vaultId: string, enabled = true) =>
  useQuery({
    queryKey: ontologyKeys.vault(vaultId),
    queryFn: () => call(getVaultOntology({ path: { vault_id: vaultId } })),
    enabled,
  });

/** Every decision answers with the whole ontology, so the list updates without a second request. */
const useDecision = <V>(vaultId: string, fn: (v: V) => Promise<OntologyOut>) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: (o) => {
      qc.setQueryData(ontologyKeys.vault(vaultId), o);
      // entity_types and the kind filters show the review too
      return qc.invalidateQueries({ queryKey: entityKeys.all });
    },
  });
};

export const useDecideKind = (vaultId: string) =>
  useDecision(vaultId, ({ kind, decision }: { kind: string; decision: KindDecision }) =>
    call(decideVaultOntologyKind({ path: { vault_id: vaultId }, body: { kind, decision } })),
  );

export const useDecideLink = (vaultId: string) =>
  useDecision(vaultId, (v: { from_kind: string; label: string; to_kind: string; decision: LinkDecision }) =>
    call(decideVaultOntologyLink({ path: { vault_id: vaultId }, body: v })),
  );

export const useSuggestOntology = (vaultId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => call(suggestVaultOntology({ path: { vault_id: vaultId } })),
    onSuccess: (out) => qc.setQueryData(ontologyKeys.vault(vaultId), out.mode === "model" ? out.ontology : out.profile),
  });
};
