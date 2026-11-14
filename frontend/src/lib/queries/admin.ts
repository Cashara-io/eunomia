import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  completeAccessReview,
  decideAccessReview,
  exportAccessReview,
  exportAuditChain,
  getAccessReview,
  listAccessReviews,
  listPeople,
  listPersonTokens,
  listWorkspaceAudit,
  revokePersonToken,
  setPersonAdmin,
  setPersonSuspended,
  startAccessReview,
  verifyAuditChain,
  type AccessReview,
  type AuditCategory,
  type PeopleReport,
} from "@/lib/gen";
import { call } from "./client";

export const adminKeys = {
  all: ["admin"] as const,
  audit: (category: AuditCategory, limit: number, offset: number) => [...adminKeys.all, "audit", category, limit, offset] as const,
  people: () => [...adminKeys.all, "people"] as const,
  tokens: (user_id: string) => [...adminKeys.all, "tokens", user_id] as const,
  verify: () => [...adminKeys.all, "verify"] as const,
  reviews: () => [...adminKeys.all, "reviews"] as const,
  review: (id: string) => [...adminKeys.all, "reviews", id] as const,
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

/** Walks the security log's hash chain (admins only). Runs when asked (`refetch`), never on its own. */
export const useVerifyAudit = () =>
  useQuery({ queryKey: adminKeys.verify(), queryFn: () => call(verifyAuditChain()), enabled: false, retry: false });

/** Saves `data` as a JSON file named `name`. */
export function saveJson(data: unknown, name: string) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2) + "\n"], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

/** The security log for a range of days, with its checkpoints, as a file anyone can verify offline. */
export async function downloadAuditExport(from: string, to: string) {
  // "until" a day includes that day: the server's end is exclusive
  const end = to ? new Date(Date.parse(`${to}T00:00:00Z`) + 86_400_000).toISOString().slice(0, 10) : undefined;
  const data = await call(exportAuditChain({ query: { from: from || undefined, to: end } }));
  saveJson(data, `eunomia-security-log-${from || "start"}-to-${to || "now"}.json`);
  return data;
}

/** Access reviews, newest first (admins only). */
export const useAccessReviews = () => useQuery({ queryKey: adminKeys.reviews(), queryFn: () => call(listAccessReviews()) });

/** One access review with its snapshot and decisions. */
export const useAccessReview = (id: string | null) =>
  useQuery({ queryKey: adminKeys.review(id ?? ""), queryFn: () => call(getAccessReview({ path: { review_id: id! } })), enabled: !!id });

function useReviewMutation<V>(fn: (v: V) => Promise<AccessReview>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: fn,
    onSuccess: (review) => {
      qc.setQueryData(adminKeys.review(review.id), review);
      void qc.invalidateQueries({ queryKey: adminKeys.reviews(), exact: true });
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
    },
  });
}

/** Start a review: the server snapshots who has access to what. */
export const useStartAccessReview = () => useReviewMutation(() => call(startAccessReview()));

/** Record keep, remove or suspend for one person. */
export const useDecideAccessReview = () =>
  useReviewMutation(({ review_id, user_id, decision, note }: { review_id: string; user_id: string; decision: string; note?: string }) =>
    call(decideAccessReview({ path: { review_id, user_id }, body: { decision, note: note ?? "" } }))
  );

/** Complete a review; it can't change afterwards. */
export const useCompleteAccessReview = () => useReviewMutation((review_id: string) => call(completeAccessReview({ path: { review_id } })));

/** A completed review with its hashes, as a file. */
export async function downloadAccessReview(review_id: string) {
  const data = await call(exportAccessReview({ path: { review_id } }));
  saveJson(data, `eunomia-access-review-${review_id.replace(/[^a-z0-9]+/gi, "-")}.json`);
  return data;
}
