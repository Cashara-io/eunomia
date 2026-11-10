import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { API_URL, ApiError } from "@/lib/api";

const value = z.union([z.string(), z.number(), z.boolean(), z.null()]);
export const columnSchema = z.object({
  name: z.string(),
  datatype: z.string(),
  unit: z.string().nullable(),
});
const cellSchema = z.object({ value, unit: z.string().nullable() });
const rowSchema = z.object({
  id: z.string(),
  name: z.string(),
  values: z.record(z.string(), cellSchema),
  source_count: z.number(),
});
const listSchema = z.object({
  rows: z.array(rowSchema),
  total: z.number(),
  offset: z.number(),
  limit: z.number(),
  columns: z.array(columnSchema),
  kind_approved: z.boolean(),
});
const propertySchema = columnSchema.extend({
  value,
  as_of: z.string().nullable(),
  recorded_at: z.string().nullable().optional(),
  source: z.string().nullable(),
  source_label: z.string(),
  source_state: z.string(),
  state: z.enum(["recorded", "missing", "stale", "conflicting", "unavailable"]),
});
const historySchema = z.object({
  id: z.string(),
  property: z.string().nullable(),
  value_num: z.number().nullable().optional(),
  value_text: z.string().nullable().optional(),
  value_bool: z.boolean().nullable().optional(),
  value_date: z.string().nullable().optional(),
  as_of: z.string(),
  recorded_at: z.string(),
  source: z.string(),
});
const linkSchema = z.object({
  id: z.string(),
  label: z.string(),
  from_id: z.string(),
  from_name: z.string(),
  to_id: z.string(),
  to_name: z.string(),
  recorded_at: z.string(),
});
const runSchema = z.object({
  value: z.number().nullable(),
  unit: z.string().nullable(),
  expr_words: z.string(),
  as_of: z.string(),
  n_entities: z.number(),
  input_properties: z.array(z.string()),
  entity_ids: z.array(z.string()),
  warnings: z.array(z.string()),
  truncated: z.boolean(),
});
export const profileSchema = z.object({
  id: z.string(),
  name: z.string(),
  kind: z.string(),
  summary: z.string(),
  kind_approved: z.boolean(),
  properties: z.array(propertySchema),
  numbers: z.array(
    z.object({
      name: z.string(),
      definition: z.string(),
      result: z.object({
        run: runSchema.optional(),
        unavailable: z.string().optional(),
      }),
    }),
  ),
  links: z.array(linkSchema),
  link_counts: z.array(
    z.object({ label: z.string(), outgoing: z.boolean(), total: z.number() }),
  ),
  history: z.array(historySchema),
  history_total: z.number().nullable(),
  sources: z.array(
    z.object({
      id: z.string(),
      record: z
        .object({
          title: z.string().nullable().optional(),
          source: z.string(),
          external_id: z.string(),
          payload: z.unknown(),
        })
        .nullable(),
      state: z.string(),
      document: z
        .object({
          id: z.string(),
          filename: z.string(),
          row: z
            .object({
              line: z.number(),
              cells: z.record(z.string(), z.string()),
            })
            .nullable(),
          note: z.string(),
        })
        .optional(),
    }),
  ),
  memories: z.array(
    z.object({
      id: z.string(),
      text: z.string(),
      recorded_at: z.string(),
      type: z.string(),
    }),
  ),
  more: z.object({
    history: z.boolean(),
    links: z.boolean(),
    memories: z.boolean(),
  }),
  offset: z.number(),
  warnings: z.array(z.string()),
});
export type ObjectRow = z.infer<typeof rowSchema>;
export type ObjectProfile = z.infer<typeof profileSchema>;
export type ObjectColumn = z.infer<typeof columnSchema>;
async function read<T>(
  path: string,
  schema: z.ZodType<T>,
  signal: AbortSignal,
): Promise<T> {
  const res = await fetch(`${API_URL}${path}`, {
    credentials: "include",
    signal,
  });
  const body: unknown = await res.json();
  if (!res.ok) {
    const problem = z
      .object({ detail: z.string().optional(), code: z.string().optional() })
      .safeParse(body);
    throw new ApiError(
      res.status,
      problem.success ? (problem.data.code ?? "http.error") : "http.error",
      problem.success
        ? (problem.data.detail ?? "Could not load objects.")
        : "Could not load objects.",
    );
  }
  return schema.parse(body);
}
export function useObjects(
  space: string,
  params: URLSearchParams,
  kind: string,
) {
  const query = new URLSearchParams({ kind, limit: "25" });
  for (const key of ["search", "sort", "descending", "filter", "op", "value"])
    if (params.get(key)) query.set(key, params.get(key) ?? "");
  if (query.get("op") !== "missing" && !query.get("value")) query.delete("op");
  const page = Math.max(1, Number(params.get("page")) || 1);
  query.set("offset", String((page - 1) * 25));
  const path = `/api/vaults/${encodeURIComponent(space)}/ontology/objects?${query}`;
  return useQuery({
    retryOnMount: false,
    queryKey: ["objects", space, query.toString()],
    queryFn: ({ signal }) => read(path, listSchema, signal),
    enabled: !!kind,
  });
}
export function useObjectProfile(space: string, id: string, offset = 0, enabled = true) {
  return useQuery({
    retryOnMount: false,
    queryKey: ["object-profile", space, id, offset],
    queryFn: ({ signal }) =>
      read(
        `/api/vaults/${encodeURIComponent(space)}/ontology/objects/${encodeURIComponent(id)}?offset=${offset}`,
        profileSchema,
        signal,
      ),
    enabled: !!id && enabled,
  });
}

export function useObjectKinds(space: string) {
  return useQuery({
    retryOnMount: false,
    queryKey: ["object-kinds", space],
    queryFn: ({ signal }) =>
      read(
        `/api/vaults/${encodeURIComponent(space)}/ontology/objects`,
        z.object({
          kinds: z.array(z.object({ kind: z.string(), entities: z.number() })),
        }),
        signal,
      ),
  });
}
