import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { API_URL, ApiError } from "@/lib/api";

const schema = z.object({
  events: z.array(z.object({
    seq: z.number(), at: z.string(), action: z.string(), actor_name: z.string(),
    definition_type: z.string(), kind: z.string(), name: z.string(),
  })),
});
export type LatestActivity = z.infer<typeof schema>["events"][number];

/** The newest ontology event, for the stage's activity strip. The live stream invalidates it with the rest of the activity reads. */
export function useLatestActivity(space: string, enabled: boolean) {
  return useQuery({
    queryKey: ["ontology-activity", space, "stage-strip"],
    enabled,
    retryOnMount: false,
    queryFn: async ({ signal }) => {
      const response = await fetch(
        `${API_URL}/api/vaults/${encodeURIComponent(space)}/ontology/activity?newest=true&limit=1`,
        { credentials: "include", signal },
      );
      if (!response.ok) throw new ApiError(response.status, "http.error", "Could not load activity.");
      return schema.parse(await response.json()).events[0] ?? null;
    },
  });
}
