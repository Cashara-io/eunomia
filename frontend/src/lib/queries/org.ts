import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  acceptOrgInvitation,
  createOrgInvitation,
  getOrg,
  getOrgPlan,
  getOrgSeats,
  listOrgInvitations,
  lookupOrgInvitation,
  renameOrg,
  resendOrgInvitation,
  revokeOrgInvitation,
  setOrgSecurity,
  type OrgInviteRole,
  type OrgOut,
} from "@/lib/gen";
import { call } from "./client";

export const orgKeys = {
  all: ["org"] as const,
  invitations: () => [...orgKeys.all, "invitations"] as const,
  seats: () => [...orgKeys.all, "seats"] as const,
  plan: () => [...orgKeys.all, "plan"] as const,
  settings: () => [...orgKeys.all, "settings"] as const,
  lookup: (token: string) => [...orgKeys.all, "lookup", token] as const,
};

/** The organisation's open invitations (owner or admin). Never carries a link. */
export const useOrgInvitations = (enabled = true) =>
  useQuery({ queryKey: orgKeys.invitations(), queryFn: () => call(listOrgInvitations()), enabled });

/** Active members, suspended members and pending invitations (owner or admin). */
export const useOrgSeats = (enabled = true) => useQuery({ queryKey: orgKeys.seats(), queryFn: () => call(getOrgSeats()), enabled });

/** The organisation's plan, seats and status (any member). Nulls when no plan was set on it (a self-hosted install).
 * Polled, so a workspace the billing service suspends or resumes shows it without a reload. */
export const useOrgPlan = (enabled = true) =>
  useQuery({ queryKey: orgKeys.plan(), queryFn: () => call(getOrgPlan()), enabled, staleTime: 15_000, refetchInterval: 30_000 });

function useOrgMutation<V, R>(fn: (v: V) => Promise<R>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: orgKeys.all });
      void qc.invalidateQueries({ queryKey: ["admin", "audit"] });
    },
  });
}

/** Invite an email. The answer carries the link, once. */
export const useCreateInvitation = () =>
  useOrgMutation(({ email, role }: { email: string; role: OrgInviteRole }) => call(createOrgInvitation({ body: { email, role } })));

/** A new link for an open invitation; the old one stops working. */
export const useResendInvitation = () => useOrgMutation((invitation_id: string) => call(resendOrgInvitation({ path: { invitation_id } })));

/** Withdraw an open invitation. */
export const useRevokeInvitation = () => useOrgMutation((invitation_id: string) => call(revokeOrgInvitation({ path: { invitation_id } })));

/** What an invitation link is for (public). */
export const useInvitationLookup = (token: string | null) =>
  useQuery({
    queryKey: orgKeys.lookup(token ?? ""),
    queryFn: () => call(lookupOrgInvitation({ body: { token: token ?? "" } })),
    enabled: !!token,
    retry: false,
  });

/** Accept with the signed-in account. */
export const useAcceptInvitation = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (token: string) => call(acceptOrgInvitation({ body: { token } })),
    onSuccess: () => qc.invalidateQueries(),
  });
};

/** The organisation's name and its own two-step verification policy (owner or admin). */
export const useOrg = (enabled = true) => useQuery({ queryKey: orgKeys.settings(), queryFn: () => call(getOrg()), enabled });

function useOrgSettingsMutation<V>(fn: (v: V) => Promise<OrgOut>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: (org) => {
      qc.setQueryData(orgKeys.settings(), org);
      // the people list shows who still has to enrol, and the audit log has a new row
      void qc.invalidateQueries({ queryKey: ["admin"] });
    },
  });
}

/** Rename the organisation (its owner). */
export const useRenameOrg = () => useOrgSettingsMutation((name: string) => call(renameOrg({ body: { name } })));

/** Require two-step verification of the organisation's members, or stop. `mfa_code` answers a step-up. */
export const useSetOrgSecurity = () =>
  useOrgSettingsMutation(({ require_mfa, mfa_code }: { require_mfa: boolean; mfa_code?: string }) =>
    call(setOrgSecurity({ body: { require_mfa, ...(mfa_code ? { mfa_code } : {}) } }))
  );
