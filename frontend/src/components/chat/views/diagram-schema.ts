import { z } from "zod";
export const diagramViewSchema = z.object({
  view: z.literal("diagram"),
  text: z.string(),
  diagram: z.object({
    diagram_type: z.enum(["neighbourhood", "path", "impact", "kinds"]),
    vault: z.string(),
    title: z.string(),
    height: z.number().int().positive(),
    total_nodes: z.number().int().nonnegative(),
    total_edges: z.number().int().nonnegative(),
    omitted_edges: z.number().int().nonnegative(),
    partial: z.boolean(),
    total_partial: z.boolean(),
    notice: z.string().nullable(),
    nodes: z
      .array(
        z.object({
          id: z.string(),
          name: z.string(),
          kind: z.string(),
          focus: z.boolean(),
          emphasis: z.boolean(),
          x: z.number(),
          y: z.number(),
          count: z.number().optional(),
          metrics: z.array(
            z.object({
              metric: z.string(),
              value: z.number().nullable(),
              unit: z.string().nullable(),
              n_entities: z.number(),
            }),
          ),
        }),
      )
      .max(12),
    edges: z
      .array(
        z.object({
          id: z.string(),
          from: z.string(),
          to: z.string(),
          label: z.string(),
          count: z.number().optional(),
        }),
      )
      .max(24),
    groups: z.array(
      z.object({ kind: z.string(), count: z.number().int().nonnegative() }),
    ),
  }),
});
export type DiagramView = z.infer<typeof diagramViewSchema>;
