"use client";
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { onlineManager, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { listMembers } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import ErrorLine from "@/components/ErrorLine";
import { objectMissing, spaceDenied } from "./connection-access";
import { clearObjectReturn } from "./use-object-return";

type Access =
  | { kind: "confirmed" }
  | { kind: "checking"; attempt: number }
  | { kind: "unknown"; attempt: number }
  | { kind: "denied" };

const CHECK_DEADLINE_MS = 10_000;

const ConfirmedAccess = createContext(false);
export const useConfirmedSpaceAccess = () => useContext(ConfirmedAccess);
export function ProtectedSpaceContent({ children }: { children: ReactNode }) {
  return useConfirmedSpaceAccess() ? children : null;
}

const MissingObjects = createContext<ReadonlySet<string>>(new Set());
export const useMissingSpaceObjects = () => useContext(MissingObjects);

export type ProtectedQueries = {
  keys: readonly string[];
  objectKeys: readonly string[];
  checkOnMount?: boolean;
};
/**
 * Protected data is anything that came from the server for that space: objects,
 * names in results, values, sources, rows, file names, counts, activity,
 * suggestions and recents built from results. It must never be in the DOM
 * (text or attributes) unless access is CONFIRMED.
 *
 * The user's own input is not protected data: the text they typed into a search
 * box, a filter they chose, or an id they selected. It may stay in the URL and
 * in the input. A search box may show what the user typed; it may show no results,
 * suggestions or matches unless access is confirmed.
 *
 * The persistent Ontology layout owns one gate for the selected space. Nested
 * readers share it, including across Back and Forward. A fresh gate distrusts
 * warm caches: only a successful authorised read in this mount can open it.
 */
export default function SpaceAccess({ space, children, queries: protectedQueries }: { space: string; children: ReactNode; queries: ProtectedQueries }) {
  const cache = useQueryClient();
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      queueMicrotask(() => {
        // Strict Mode mounts again immediately. A real departure leaves only inactive reads.
        if (mounted.current) return;
        const scoped = { predicate: (q: { queryKey: readonly unknown[]; getObserversCount: () => number }) =>
          protectedQueries.keys.includes(String(q.queryKey[0])) && q.queryKey[1] === space && q.getObserversCount() === 0 };
        void cache.cancelQueries(scoped);
        cache.removeQueries(scoped);
      });
    };
  }, [cache, space, protectedQueries]);
  const [access, setAccess] = useState<Access>({ kind: "checking", attempt: 0 });
  const [missing, setMissing] = useState<ReadonlySet<string>>(new Set());
  const retry = useRef(() => {});
  useEffect(() => {
    if (access.kind !== "denied") return;
    // A URL update can render existing observers before denial commits. Purge those entries after unmount too.
    const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey.includes(space) };
    void cache.cancelQueries(scoped);
    cache.removeQueries(scoped);
  }, [access.kind, cache, space]);
  useEffect(() => {
    const queries = cache.getQueryCache();
    const controller = new AbortController();
    const membershipKey: QueryKey = ["receipt-access", space];
    // An idle key lets every screen request a shared recheck without a mount-time membership read.
    queries.build(cache, { queryKey: membershipKey });
    const seen = new Set<unknown>();
    const pendingMissing = new Set<string>();
    let state: Access = { kind: "checking", attempt: 0 };
    let generation = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function transition(next: Access) {
      state = next;
      setAccess(next);
      clearTimeout(timer);
    }
    function deny() {
      if (state.kind === "denied" || controller.signal.aborted) return;
      generation++;
      transition({ kind: "denied" });
      pendingMissing.clear();
      clearObjectReturn(space);
      setMissing(new Set());
      const params = new URLSearchParams({ space });
      window.history.replaceState(null, "", `${window.location.pathname}?${params}`);
      const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey.includes(space) };
      void cache.cancelQueries(scoped);
      cache.removeQueries(scoped);
    }
    function confirm(id?: string) {
      if (id) pendingMissing.add(id);
      if (controller.signal.aborted || state.kind === "checking" && state.attempt > 0) return;
      const attempt = state.kind === "unknown" ? state.attempt + 1 : 1;
      const current = ++generation;
      transition({ kind: "checking", attempt });
      // ListMembers permits every member and checks the requested vault, not an entity.
      // A membership check that never answers must not leave "Checking access" up for ever:
      // after the deadline this attempt is aborted, and the catch below moves to unknown and retries.
      const attemptAbort = new AbortController();
      const abortAttempt = () => attemptAbort.abort();
      controller.signal.addEventListener("abort", abortAttempt, { once: true });
      const deadline = setTimeout(abortAttempt, CHECK_DEADLINE_MS);
      const request = (signal = attemptAbort.signal) => call(listMembers({ path: { vault_id: space }, signal }));
      const read = cache.fetchQuery({ queryKey: membershipKey, queryFn: ({ signal }) => request(AbortSignal.any([signal, attemptAbort.signal])), staleTime: 0, retry: false, networkMode: "always" });
      void read.then(() => {
        if (controller.signal.aborted || current !== generation) return;
        if (!onlineManager.isOnline()) throw new Error("Offline membership check");
        const ids = [...pendingMissing];
        pendingMissing.clear();
        setMissing(prev => new Set([...prev, ...ids]));
        for (const id of ids) {
          const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => protectedQueries.objectKeys.includes(String(q.queryKey[0])) && q.queryKey[1] === space && q.queryKey[2] === id };
          void cache.cancelQueries(scoped);
          cache.removeQueries(scoped);
        }
        transition({ kind: "confirmed" });
      }).catch(error => {
        if (controller.signal.aborted || current !== generation) return;
        if (spaceDenied(error, true)) deny();
        else {
          transition({ kind: "unknown", attempt });
          timer = setTimeout(() => confirm(), Math.min(30_000, 2_000 * 2 ** Math.min(attempt - 1, 4)));
        }
      }).finally(() => {
        clearTimeout(deadline);
        controller.signal.removeEventListener("abort", abortAttempt);
      });
    }
    retry.current = () => confirm();
    function check(query: { queryKey: readonly unknown[]; state: { error: unknown; fetchFailureReason: unknown } }) {
      if (!query.queryKey.includes(space)) return;
      const objectRead = protectedQueries.objectKeys.includes(String(query.queryKey[0]));
      for (const error of [query.state.error, query.state.fetchFailureReason]) {
        if (state.kind === "denied") return;
        if (!error || seen.has(error)) continue;
        seen.add(error);
        if (spaceDenied(error, !objectRead)) deny();
        else if (objectRead && objectMissing(error) && typeof query.queryKey[2] === "string") confirm(query.queryKey[2]);
      }
    }
    const unsubscribe = queries.subscribe(event => {
      if (event.type !== "updated") return;
      if (event.query.queryKey[0] === membershipKey[0] && event.query.queryKey[1] === space && event.action.type === "invalidate") {
        confirm();
      } else if (event.query.queryKey[0] === "vaults" && event.query.queryKey[1] === "list" && event.action.type === "success") {
        if (state.kind === "denied" || state.kind === "unknown") confirm();
      } else {
        check(event.query);
        const initial = state.kind === "checking" && state.attempt === 0;
        const scoped = protectedQueries.keys.includes(String(event.query.queryKey[0])) && event.query.queryKey[1] === space;
        // setQueryData is a cache write, not proof of a newly authorised response.
        if (initial && scoped && event.action.type === "success" && !event.action.manual) transition({ kind: "confirmed" });
        else if (initial && scoped && event.query.state.fetchFailureReason) confirm();
      }
    });
    for (const query of queries.getAll()) check(query);
    if (protectedQueries.checkOnMount) confirm();
    else {
      // Cached successes from a previous area mount do not authorise this one.
      timer = setTimeout(() => {
        transition({ kind: "unknown", attempt: 0 });
        timer = setTimeout(() => confirm(), 2_000);
      }, CHECK_DEADLINE_MS);
      void cache.invalidateQueries({ predicate: q => protectedQueries.keys.includes(String(q.queryKey[0])) && q.queryKey[1] === space }, { cancelRefetch: false });
    }
    const recheck = () => { if (protectedQueries.checkOnMount) confirm(); };
    window.addEventListener("focus", recheck);
    window.addEventListener("offline", recheck);
    window.addEventListener("online", recheck);
    return () => {
      window.removeEventListener("focus", recheck);
      window.removeEventListener("offline", recheck);
      window.removeEventListener("online", recheck);
      unsubscribe();
      clearTimeout(timer);
      controller.abort();
      void cache.cancelQueries({ queryKey: membershipKey });
    };
  }, [cache, space, protectedQueries]);
  return (
    <ConfirmedAccess.Provider value={access.kind === "confirmed"}>
      {access.kind === "denied" ? (
        <ErrorLine action={<button className="btn btn-sm" onClick={() => retry.current()}>Retry space access</button>}>
          This information is unavailable to you in this space. Ask a space owner for access.
        </ErrorLine>
      ) : (
        <>
          {access.kind === "checking" && <p role="status">Checking access</p>}
          {access.kind === "unknown" && (
            <ErrorLine action={<button className="btn btn-sm" onClick={() => retry.current()}>Try again</button>}>
              Couldn&apos;t confirm access. Try again
            </ErrorLine>
          )}
          <MissingObjects.Provider value={missing}>{children}</MissingObjects.Provider>
        </>
      )}
    </ConfirmedAccess.Provider>
  );
}
