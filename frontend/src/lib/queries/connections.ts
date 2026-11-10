import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { API_URL, ApiError } from "@/lib/api";

export const connectionNodeSchema = z.object({
  id: z.string(),
  name: z.string(),
  kind: z.string(),
  summary: z.string().nullable().optional(),
  facts: z.array(z.string()).default([]),
});
export const connectionEdgeSchema = z.object({
  id: z.string(),
  from: z.string(),
  label: z.string(),
  to: z.string(),
});
export const neighbourhoodSchema = z.object({
  vault: z.string(),
  depth: z.number(),
  nodes: z.array(connectionNodeSchema),
  edges: z.array(connectionEdgeSchema),
  truncated: z.boolean(),
  partial: z.boolean().optional(),
});
export type ConnectionNode = z.infer<typeof connectionNodeSchema>;
export type ConnectionEdge = z.infer<typeof connectionEdgeSchema>;
export type Neighbourhood = z.infer<typeof neighbourhoodSchema>;
async function tool<T>(
  name: string,
  args: Record<string, unknown>,
  schema: z.ZodType<T>,
  signal: AbortSignal,
) {
  const res = await fetch(`${API_URL}/api/tools/${name}`, {
    method: "POST",
    credentials: "include",
    signal,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  const body: unknown = await res.json();
  const problem = z
    .object({ error: z.string().optional(), code: z.string().optional() })
    .safeParse(body);
  if (!res.ok || (problem.success && problem.data.error)) {
    const code = problem.success
      ? (problem.data.code ?? "http.error")
      : "http.error";
    throw new ApiError(
      !res.ok ? res.status :
        ["vault.forbidden", "auth.scope", "tenant.denied", "auth.forbidden"].includes(code) ? 403 :
        ["entity.not_found", "resource.not_found", "vault.not_found"].includes(code) ? 404 : 500,
      code,
      "Connections unavailable",
    );
  }
  return schema.parse(body);
}
export function useNeighbourhood(space: string, id: string, depth: number) {
  return useQuery({
    queryKey: ["connections", space, id, depth],
    gcTime: 0,
    refetchOnWindowFocus: false,
    enabled: !!id,
    queryFn: async ({ signal }) => {
      const data = await tool(
        "entities_neighbourhood",
        { entity_id: id, vault_id: space, depth },
        neighbourhoodSchema,
        signal,
      );
      if (data.vault !== space)
        throw new ApiError(404, "scope.mismatch", "Unavailable in this space");
      return data;
    },
    retry: false,
  });
}
const searchSchema = z.object({
  results: z.array(connectionNodeSchema),
  has_more: z.boolean().optional(),
});
export function useConnectionSearch(
  space: string,
  query: string,
  kind: string,
  offset: number,
  enabled = true,
) {
  return useQuery({
    queryKey: ["connection-search", space, query, kind, offset],
    enabled,
    queryFn: ({ signal }) =>
      tool(
        "entities_search",
        {
          query,
          vault_id: space,
          ...(kind ? { kind } : {}),
          limit: 20,
          offset,
        },
        searchSchema,
        signal,
      ),
    retry: false,
  });
}
