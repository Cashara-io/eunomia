import { useQuery } from "@tanstack/react-query";
import { getToolUsage, listAudit } from "@/lib/gen";
import { call } from "./client";

export const auditKeys = {
  all: ["audit"] as const,
  list: (limit: number, offset: number) => [...auditKeys.all, "list", limit, offset] as const,
  usage: (hours: number) => [...auditKeys.all, "tool-usage", hours] as const,
};

export const useAudit = (limit = 50, offset = 0) =>
  useQuery({ queryKey: auditKeys.list(limit, offset), queryFn: () => call(listAudit({ query: { limit, offset } })) });

/** Tool-call counts over the last `hours` hours: the caller's own, or the whole org for an admin. */
export const useToolUsage = (hours: number) =>
  useQuery({
    queryKey: auditKeys.usage(hours),
    queryFn: () => call(getToolUsage({ query: { hours } })),
    // counters move while the tab is open
    refetchInterval: 60_000,
  });
