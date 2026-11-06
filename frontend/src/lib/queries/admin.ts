import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listPeople, listWorkspaceAudit, setPersonAdmin, setPersonSuspended, type AuditCategory } from "@/lib/gen";
import { call } from "./client";

export const adminKeys = {
  all: ["admin"] as const,
  audit: (category: AuditCategory, limit: number, offset: number) => [...adminKeys.all, "audit", category, limit, offset] as const,
  people: () => [...adminKeys.all, "people"] as const,
};

/** The workspace activity log (admins only): sign-ins, tokens, app connections, space and admin changes, newest first. */
export const useWorkspaceAudit = (category: AuditCategory, limit = 50, offset = 0) =>
  useQuery({
    queryKey: adminKeys.audit(category, limit, offset),
    queryFn: () => call(listWorkspaceAudit({ query: { category, limit, offset } })),
    placeholderData: keepPreviousData,
  });

/** Every account in the workspace plus the sign-up policy (admins only). */
export const usePeople = () => useQuery({ queryKey: adminKeys.people(), queryFn: () => call(listPeople()) });

/** Make someone an admin, or take it back. The server answers with the updated people list. */
export function useSetAdmin() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ user_id, admin }: { user_id: string; admin: boolean }) => call(setPersonAdmin({ path: { user_id }, body: { admin } })),
    onSuccess: (report) => {
      qc.setQueryData(adminKeys.people(), report);
      // the audit log has a new row
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
    },
  });
}

/** Suspend or restore an account. The server answers with the updated people list. */
export function useSetSuspended() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ user_id, suspended }: { user_id: string; suspended: boolean }) =>
      call(setPersonSuspended({ path: { user_id }, body: { suspended } })),
    onSuccess: (report) => {
      qc.setQueryData(adminKeys.people(), report);
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
    },
  });
}
