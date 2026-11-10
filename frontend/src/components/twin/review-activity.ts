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

// The inbox and detail share one scoped activity record. Evidence never waits for it.
export function useReviewActivity(vault: string, d: Definition) {
  return useQuery({
    queryKey: ["ontology-activity", vault, "decision-receipt", d.id],
    queryFn: async () => {
      const params = new URLSearchParams({
        definition_type: d.type, kind: d.kind, name: d.name,
        newest: "true", limit: "30",
      });
      const response = await fetch(
        `${API_URL}/api/vaults/${encodeURIComponent(vault)}/ontology/activity?${params}`,
        { credentials: "include" },
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
