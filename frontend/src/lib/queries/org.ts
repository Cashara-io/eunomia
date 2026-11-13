import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  acceptOrgInvitation,
  createOrgInvitation,
  getOrgSeats,
  listOrgInvitations,
  lookupOrgInvitation,
  resendOrgInvitation,
  revokeOrgInvitation,
  type OrgInviteRole,
} from "@/lib/gen";
import { call } from "./client";

export const orgKeys = {
  all: ["org"] as const,
  invitations: () => [...orgKeys.all, "invitations"] as const,
  seats: () => [...orgKeys.all, "seats"] as const,
  lookup: (token: string) => [...orgKeys.all, "lookup", token] as const,
};

/** The organisation's open invitations (owner or admin). Never carries a link. */
export const useOrgInvitations = (enabled = true) =>
  useQuery({ queryKey: orgKeys.invitations(), queryFn: () => call(listOrgInvitations()), enabled });

/** Active members, suspended members and pending invitations (owner or admin). */
export const useOrgSeats = (enabled = true) => useQuery({ queryKey: orgKeys.seats(), queryFn: () => call(getOrgSeats()), enabled });

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
