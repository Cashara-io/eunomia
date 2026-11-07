// A memory space's ontology: the kinds and link types it uses, and what its owners decided about them
// (backend `routers/ontology.rs`). Deciding works only from a browser session.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  decideVaultOntologyKind,
  decideVaultOntologyLink,
  decideVaultOntologyMetric,
  decideVaultOntologyProperty,
  getVaultOntology,
  suggestVaultOntology,
  type KindDecision,
  type LinkDecision,
  type MetricDecision,
  type OntologyOut,
  type PropertyDecision,
} from "@/lib/gen";
import type { MetricRun } from "@/components/chat/views/types";
import { call } from "./client";
import { entityKeys } from "./entities";
import { invoke } from "./tools";

export type { KindItem, LinkItem, MetricItem, OntologyOut, PropertyItem, SuggestOut, ViewStatus } from "@/lib/gen";

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

export const useDecideProperty = (vaultId: string) =>
  useDecision(vaultId, (v: { kind: string; name: string; decision: PropertyDecision }) =>
    call(decideVaultOntologyProperty({ path: { vault_id: vaultId }, body: v })),
  );

export const useDecideMetric = (vaultId: string) =>
  useDecision(vaultId, (v: { kind: string; name: string; decision: MetricDecision }) =>
    call(decideVaultOntologyMetric({ path: { vault_id: vaultId }, body: v })),
  );

/** Runs a metric once, approved or not, to see its number before deciding. A refusal is a value. */
export const useTryMetric = (vaultId: string) =>
  useMutation({
    mutationFn: ({ kind, name }: { kind: string; name: string }) =>
      invoke<MetricRun | { error: string }>("metric_eval", { metric: name, kind, vault_id: vaultId, allow_unapproved: true }),
  });

export const useSuggestOntology = (vaultId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => call(suggestVaultOntology({ path: { vault_id: vaultId } })),
    onSuccess: (out) => qc.setQueryData(ontologyKeys.vault(vaultId), out.mode === "model" ? out.ontology : out.profile),
  });
};
