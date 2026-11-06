import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  listPeople,
  listPersonTokens,
  listWorkspaceAudit,
  revokePersonToken,
  setPersonAdmin,
  setPersonSuspended,
  type AuditCategory,
  type PeopleReport,
} from "@/lib/gen";
import { call } from "./client";

export const adminKeys = {
  all: ["admin"] as const,
  audit: (category: AuditCategory, limit: number, offset: number) => [...adminKeys.all, "audit", category, limit, offset] as const,
  people: () => [...adminKeys.all, "people"] as const,
  tokens: (user_id: string) => [...adminKeys.all, "tokens", user_id] as const,
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

/** Someone's API tokens (admins only): names and dates, never the secret. Loads only while `enabled`. */
export const usePersonTokens = (user_id: string, enabled = true) =>
  useQuery({ queryKey: adminKeys.tokens(user_id), queryFn: () => call(listPersonTokens({ path: { user_id } })), enabled });

/** Revoke someone's API token. The server answers with their remaining tokens. */
export function useRevokePersonToken() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ user_id, token_id }: { user_id: string; token_id: string }) => call(revokePersonToken({ path: { user_id, token_id } })),
    onSuccess: (left, { user_id }) => {
      qc.setQueryData(adminKeys.tokens(user_id), left);
      // the count in the people list follows at once
      qc.setQueryData<PeopleReport>(adminKeys.people(), (r) =>
        r ? { ...r, people: r.people.map((p) => (p.id === user_id ? { ...p, tokens: left.length } : p)) } : r
      );
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
      // an admin may revoke their own token here: Settings > API tokens follows
      void qc.invalidateQueries({ queryKey: ["auth", "tokens"] });
    },
  });
}
