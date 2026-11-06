import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { listPeople, listWorkspaceAudit, type AuditCategory } from "@/lib/gen";
import { call } from "./client";

export const adminKeys = {
  all: ["admin"] as const,
  audit: (category: AuditCategory, limit: number, offset: number) => [...adminKeys.all, "audit", category, limit, offset] as const,
  people: () => [...adminKeys.all, "people"] as const,
};

/** The workspace activity log (admins only): sign-ins, tokens, app connections and space changes, newest first. */
export const useWorkspaceAudit = (category: AuditCategory, limit = 50, offset = 0) =>
  useQuery({
    queryKey: adminKeys.audit(category, limit, offset),
    queryFn: () => call(listWorkspaceAudit({ query: { category, limit, offset } })),
    placeholderData: keepPreviousData,
  });

/** Every account in the workspace plus the sign-up policy (admins only). */
export const usePeople = () => useQuery({ queryKey: adminKeys.people(), queryFn: () => call(listPeople()) });
