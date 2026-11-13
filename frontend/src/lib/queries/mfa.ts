import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  confirmMfaChallengeEnrolment,
  confirmMfaEnrolment,
  disableMfa,
  getMfa,
  getServerSecurity,
  regenerateRecoveryCodes,
  setServerSecurity,
  startMfaChallengeEnrolment,
  startMfaEnrolment,
  verifyMfaChallenge,
} from "@/lib/gen";
import type { Me } from "@/lib/types";
import { authKeys } from "./auth";
import { call } from "./client";

export const mfaKeys = {
  status: () => [...authKeys.all, "mfa"] as const,
  security: () => ["admin", "security"] as const,
};

export const useMfaStatus = () => useQuery({ queryKey: mfaKeys.status(), queryFn: () => call(getMfa()) });

function useMfaChange<V, R>(fn: (v: V) => Promise<R>) {
  const qc = useQueryClient();
  return useMutation({ mutationFn: fn, onSuccess: () => qc.invalidateQueries({ queryKey: mfaKeys.status() }) });
}

export const useStartEnrolment = () => useMutation({ mutationFn: () => call(startMfaEnrolment()) });
export const useConfirmEnrolment = () => useMfaChange((code: string) => call(confirmMfaEnrolment({ body: { code } })));
export const useDisableMfa = () => useMfaChange((code: string) => call(disableMfa({ body: { code } })));
export const useRegenerateCodes = () => useMfaChange((code: string) => call(regenerateRecoveryCodes({ body: { code } })));

// The second sign-in step: no session yet, only the short-lived challenge cookie.
export const useStartChallengeEnrolment = () => useMutation({ mutationFn: () => call(startMfaChallengeEnrolment()) });

/** A finished second step means a new session: nothing cached under the old one may survive. */
function useSecondStep<V, R>(fn: (v: V) => Promise<R>, me: (r: R) => Me) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: (r) => {
      qc.clear();
      qc.setQueryData(authKeys.me(), me(r));
    },
  });
}
export const useVerifyChallenge = () => useSecondStep((code: string) => call(verifyMfaChallenge({ body: { code } })), (r) => r as Me);
export const useConfirmChallengeEnrolment = () =>
  useSecondStep((code: string) => call(confirmMfaChallengeEnrolment({ body: { code } })), (r) => r.me as Me);

export const useServerSecurity = (enabled = true) =>
  useQuery({ queryKey: mfaKeys.security(), queryFn: () => call(getServerSecurity()), enabled });

export function useSetServerSecurity() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { require_mfa: boolean; mfa_code?: string }) => call(setServerSecurity({ body })),
    onSuccess: (data) => qc.setQueryData(mfaKeys.security(), data),
  });
}
