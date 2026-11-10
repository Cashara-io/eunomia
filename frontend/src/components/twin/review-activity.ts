import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { API_URL } from "@/lib/api";
import type { Definition } from "./model";

const activitySchema = z.object({
  events: z.array(z.object({
    action: z.string(), revision: z.number(), actor_name: z.string(),
    at: z.string(), reason: z.string().nullable(),
  })),
});

// Receipt activity stays scoped to the selected definition. Evidence never waits for it.
export function useReviewActivity(vault: string, d: Definition) {
  return useQuery({
    queryKey: ["ontology-activity", vault, "decision-receipt", d.id],
    queryFn: async ({ signal }) => {
      const params = new URLSearchParams({
        definition_type: d.type, kind: d.kind, name: d.name,
        newest: "true", limit: "30",
      });
      const response = await fetch(
        `${API_URL}/api/vaults/${encodeURIComponent(vault)}/ontology/activity?${params}`,
        { credentials: "include", signal },
      );
      if (!response.ok) throw new Error("Activity is unavailable");
      return activitySchema.parse(await response.json());
    },
  });
}

export function proposerFallback(d: Definition) {
  return d.item.origin === "agent" ? "AI app, name unavailable"
    : d.item.origin === "human" ? "Person, name unavailable"
    : d.item.origin === "suggested" ? "Server model" : "Proposer not recorded";
}

const inboxActivitySchema = z.object({
  events: z.array(activitySchema.shape.events.element.extend({
    definition_type: z.string(), kind: z.string(), name: z.string(), target: z.string(),
  })),
});
type InboxEvent = z.infer<typeof inboxActivitySchema>["events"][number];

export function useReviewInboxActivity(vault: string, scope: string, enabled: boolean) {
  return useQuery({
    queryKey: ["ontology-activity", vault, "review-inbox", scope],
    enabled,
    queryFn: async ({ signal }) => {
      // OntologyOut has no proposer or timestamp. One activity page serves the inbox.
      // Older proposals outside this bounded page retain the honest fallback labels.
      const params = new URLSearchParams({ newest: "true", action: "proposed", limit: "200" });
      const response = await fetch(
        `${API_URL}/api/vaults/${encodeURIComponent(vault)}/ontology/activity?${params}`,
        { credentials: "include", signal },
      );
      if (!response.ok) throw new Error("Activity is unavailable");
      return inboxActivitySchema.parse(await response.json());
    },
  });
}

export function proposalEvent(events: InboxEvent[], d: Definition) {
  return events.find(event => event.action === "proposed" && event.revision === d.item.revision
    && event.definition_type === d.type && event.kind === d.kind
    && event.name === d.name);
}
