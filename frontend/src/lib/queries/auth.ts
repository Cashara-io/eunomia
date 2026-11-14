import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createToken, deleteToken, getBootstrap, getMe, listSessions, listTokens, login, logout, register, renameToken, revokeSession } from "@/lib/gen";
import type { TokenCreate } from "@/lib/gen";
import type { ApiToken, Me, Session } from "@/lib/types";
import { call } from "./client";

export const authKeys = {
  all: ["auth"] as const,
  me: () => [...authKeys.all, "me"] as const,
  bootstrap: () => [...authKeys.all, "bootstrap"] as const,
  tokens: () => [...authKeys.all, "tokens"] as const,
  sessions: () => [...authKeys.all, "sessions"] as const,
};

// Always refetch on mount: the session can end or change in another tab.
export const meQuery = () => ({ queryKey: authKeys.me(), queryFn: () => call(getMe()) as Promise<Me>, staleTime: 0 });
export const bootstrapQuery = () => ({ queryKey: authKeys.bootstrap(), queryFn: () => call(getBootstrap()), staleTime: 0 });

export const useMe = (enabled = true) => useQuery({ ...meQuery(), enabled });

export const useTokens = () =>
  useQuery({ queryKey: authKeys.tokens(), queryFn: () => call(listTokens()) as Promise<ApiToken[]>, refetchInterval: 60_000 });

export function useCreateToken() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: TokenCreate) => call(createToken({ body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: authKeys.tokens() }),
  });
}

export function useRevokeToken() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (token_id: string) => call(deleteToken({ path: { token_id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: authKeys.tokens() }),
  });
}

export function useRenameToken() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ token_id, name }: { token_id: string; name: string }) => call(renameToken({ path: { token_id }, body: { name } })),
    // show the new name at once, then confirm against the server
    onSuccess: (saved) => {
      qc.setQueryData<ApiToken[]>(authKeys.tokens(), (old) => old?.map((t) => (t.id === saved.id ? { ...t, name: saved.name } : t)));
      return qc.invalidateQueries({ queryKey: authKeys.tokens() });
    },
  });
}

export const useSessions = () =>
  useQuery({ queryKey: authKeys.sessions(), queryFn: () => call(listSessions()) as Promise<Session[]> });

export function useRevokeSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (session_id: string) => call(revokeSession({ path: { session_id } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: authKeys.sessions() }),
  });
}

/** What a correct password leads to: a session, or the second step (`verify` a code, or `enrol` first
 * because the server requires two-step verification). */
export type SignIn = { kind: "session"; me: Me } | { kind: "mfa"; step: "verify" | "enrol" };

function toSignIn(r: unknown): SignIn {
  if (r && typeof r === "object" && "mfa_required" in r) {
    return { kind: "mfa", step: (r as { mfa_required: string }).mfa_required === "enrol" ? "enrol" : "verify" };
  }
  return { kind: "session", me: r as Me };
}

// A new session means a different user: nothing cached under the old one may survive.
const useSessionMutation = <V>(fn: (v: V) => Promise<unknown>) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (v: V) => toSignIn(await fn(v)),
    onSuccess: (r) => {
      if (r.kind !== "session") return;
      qc.clear();
      qc.setQueryData(authKeys.me(), r.me);
    },
  });
};
export const useLogin = () =>
  useSessionMutation(({ email, password }: { email: string; password: string }) => call(login({ body: { email, password } })));
export const useRegister = () =>
  useSessionMutation(({ email, password }: { email: string; password: string }) => call(register({ body: { email, password } })));

export function useLogout() {
  const qc = useQueryClient();
  return useMutation({ mutationFn: () => call(logout()), onSettled: () => qc.clear() });
}
