import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { decideOauthConsent, getOauthConsent, listOauthGrants, revokeOauthGrant } from "@/lib/gen";
import type { AuthzParams, GetOauthConsentData } from "@/lib/gen";
import type { OAuthGrant } from "@/lib/types";
import { call } from "./client";

export const oauthKeys = {
  all: ["oauth"] as const,
  grants: () => [...oauthKeys.all, "grants"] as const,
  consent: (query: object) => [...oauthKeys.all, "consent", query] as const,
};

export const useOauthGrants = () =>
  useQuery({ queryKey: oauthKeys.grants(), queryFn: () => call(listOauthGrants()) as Promise<OAuthGrant[]> });

export function useRevokeOauthGrant() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (grant_id: string) => call(revokeOauthGrant({ path: { grant_id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: oauthKeys.grants() }),
  });
}

/** The consent screen's request (the authorize query string, or null before it is known). Never retried or refetched: a failure sends the user back to start. */
export const useConsent = (query: NonNullable<GetOauthConsentData["query"]> | null) =>
  useQuery({
    queryKey: oauthKeys.consent(query ?? {}),
    queryFn: () => call(getOauthConsent({ query: query ?? undefined })),
    enabled: query !== null,
    retry: false,
    staleTime: Infinity,
  });

export const useDecideConsent = () =>
  useMutation({
    mutationFn: ({ params, approve, mfa_code }: { params: AuthzParams; approve: boolean; mfa_code?: string }) =>
      call(decideOauthConsent({ body: { ...params, approve, ...(mfa_code ? { mfa_code } : {}) } })),
  });
