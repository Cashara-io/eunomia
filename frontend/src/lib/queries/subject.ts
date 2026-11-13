import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  cancelErasure,
  myDataActivity,
  eraseMyAccount,
  erasePerson,
  listErasures,
  listSubjectExports,
  makeWorkspaceOwner,
  reapplyErasures,
  setPersonLegalHold,
  startPersonExport,
  startSubjectExport,
  type ExportRow,
} from "@/lib/gen";
import { call } from "./client";
import { adminKeys } from "./admin";

/** Data-subject rights (docs/privacy/subject-rights.md): full exports, erasure, legal holds. */
export const subjectKeys = {
  exports: () => ["subject", "exports"] as const,
  activity: () => ["subject", "activity"] as const,
  erasures: () => [...adminKeys.all, "erasures"] as const,
};

/** Your data exports and the ones you asked for as an admin. Polls while one is being prepared. */
export const useSubjectExports = () =>
  useQuery({
    queryKey: subjectKeys.exports(),
    queryFn: () => call(listSubjectExports()),
    refetchInterval: (q) => ((q.state.data as ExportRow[] | undefined)?.some((e) => e.status === "pending") ? 2000 : false),
  });

export function useStartExport() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => call(startSubjectExport()),
    onSuccess: () => void qc.invalidateQueries({ queryKey: subjectKeys.exports() }),
  });
}

/** Deletes your account: the server checks the password (an account with single sign-on only needs a
 * sign-in from the last 10 minutes). `close_workspace` confirms closing a workspace you are the only member of. */
export const useEraseMe = () =>
  useMutation({
    mutationFn: (body: { password?: string; close_workspace?: boolean }) => call(eraseMyAccount({ body })),
  });

/** What has been done about your data: exports an admin made, erasure requests, holds, admin actions on your account. */
export const useMyDataActivity = () => useQuery({ queryKey: subjectKeys.activity(), queryFn: () => call(myDataActivity()) });

export function useStartPersonExport() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (user_id: string) => call(startPersonExport({ path: { user_id } })),
    onSuccess: () => void qc.invalidateQueries({ queryKey: subjectKeys.exports() }),
  });
}

export function useErasePerson() {
  const qc = useQueryClient();
  return useMutation({
    // the admin's own password (an admin with single sign-on only needs a sign-in from the last 10 minutes);
    // it erases the person from this workspace only
    mutationFn: ({ user_id, password }: { user_id: string; password?: string }) => call(erasePerson({ path: { user_id }, body: { password } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: adminKeys.people() });
      void qc.invalidateQueries({ queryKey: subjectKeys.erasures() });
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
    },
  });
}

export function useSetLegalHold() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ user_id, hold }: { user_id: string; hold: boolean }) => call(setPersonLegalHold({ path: { user_id }, body: { hold } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: adminKeys.people() });
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
    },
  });
}

/** The workspace's erasure records (admins only): counts and the post-check, never who it was. */
export const useErasures = () =>
  useQuery({
    queryKey: subjectKeys.erasures(),
    queryFn: () => call(listErasures()),
    refetchInterval: (q) => ((q.state.data as { status: string }[] | undefined)?.some((e) => e.status === "pending" || e.status === "running") ? 2000 : false),
  });

export function useReapplyErasures() {
  const qc = useQueryClient();
  return useMutation({ mutationFn: () => call(reapplyErasures()), onSuccess: () => void qc.invalidateQueries({ queryKey: subjectKeys.erasures() }) });
}

/** Makes someone the workspace's owner in place of the current one. Answers with the people list. */
export function useMakeWorkspaceOwner() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (user_id: string) => call(makeWorkspaceOwner({ path: { user_id } })),
    onSuccess: (report) => {
      qc.setQueryData(adminKeys.people(), report);
      void qc.invalidateQueries({ queryKey: [...adminKeys.all, "audit"] });
    },
  });
}

/** Server operators only: stop an erasure that is not running and clear its working record. */
export function useCancelErasure() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (erasure_id: string) => call(cancelErasure({ path: { erasure_id } })),
    onSuccess: (list) => qc.setQueryData(subjectKeys.erasures(), list),
  });
}
