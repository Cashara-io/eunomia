import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { onlineManager, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { listMembers } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import ErrorLine from "@/components/ErrorLine";
import { objectMissing, spaceDenied } from "./connection-access";

type Access =
  | { kind: "confirmed" }
  | { kind: "checking"; attempt: number }
  | { kind: "unknown"; attempt: number }
  | { kind: "denied" };

const CHECK_DEADLINE_MS = 10_000;

const ConfirmedAccess = createContext(true);
export const useConfirmedSpaceAccess = () => useContext(ConfirmedAccess);
export function ProtectedSpaceContent({ children }: { children: ReactNode }) {
  return useConfirmedSpaceAccess() ? children : null;
}

const MissingObjects = createContext<ReadonlySet<string>>(new Set());
export const useMissingSpaceObjects = () => useContext(MissingObjects);

// Stateful readers retain their query while the address bar withholds it.
const WithheldParams = createContext<URLSearchParams | null>(null);
export const useWithheldSpaceParams = () => useContext(WithheldParams);
const ActiveSpace = createContext("");
export type ProtectedQueries = {
  keys: readonly string[];
  objectKeys: readonly string[];
  checkOnMount?: boolean;
};
export default function SpaceAccess({ space, children, queries: protectedQueries }: { space: string; children: ReactNode; queries: ProtectedQueries }) {
  const parent = useContext(ActiveSpace);
  return parent === space ? children : <AccessGate key={space} space={space} queries={protectedQueries}>{children}</AccessGate>;
}
function AccessGate({ space, children, queries: protectedQueries }: { space: string; children: ReactNode; queries: ProtectedQueries }) {
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
  const [access, setAccess] = useState<Access>(protectedQueries.checkOnMount ? { kind: "checking", attempt: 1 } : { kind: "confirmed" });
  const withheld = useRef<{ pathname: string; search: string; hash: string } | null>(null);
  const [withheldParams, setWithheldParams] = useState<URLSearchParams | null>(null);
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
    const seen = new Set<unknown>();
    const pendingMissing = new Set<string>();
    let state: Access = { kind: "confirmed" };
    let generation = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function transition(next: Access) {
      if (next.kind === "checking" || next.kind === "unknown") {
        if (!withheld.current) {
          withheld.current = { pathname: window.location.pathname, search: window.location.search, hash: window.location.hash };
          setWithheldParams(new URLSearchParams(withheld.current.search));
        }
        // Both Objects and Connections keep only the space ID while access is withheld.
        window.history.replaceState(null, "", `${window.location.pathname}?${new URLSearchParams({ space })}`);
      } else {
        if (next.kind === "confirmed" && withheld.current && window.location.pathname === withheld.current.pathname && new URLSearchParams(window.location.search).get("space") === space) {
          window.history.replaceState(null, "", withheld.current.pathname + withheld.current.search + withheld.current.hash);
        }
        withheld.current = null;
        setWithheldParams(null);
      }
      state = next;
      setAccess(next);
      clearTimeout(timer);
    }
    function deny() {
      if (state.kind === "denied" || controller.signal.aborted) return;
      generation++;
      transition({ kind: "denied" });
      pendingMissing.clear();
      setMissing(new Set());
      const params = new URLSearchParams({ space });
      window.history.replaceState(null, "", `${window.location.pathname}?${params}`);
      const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey.includes(space) };
      void cache.cancelQueries(scoped);
      cache.removeQueries(scoped);
    }
    function confirm(id?: string) {
      if (id) pendingMissing.add(id);
      if (controller.signal.aborted || state.kind === "checking") return;
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
      const read = protectedQueries.checkOnMount
        ? cache.fetchQuery({ queryKey: membershipKey, queryFn: ({ signal }) => request(AbortSignal.any([signal, attemptAbort.signal])), staleTime: 0, retry: false, networkMode: "always" })
        : request();
      void read.then(() => {
        if (controller.signal.aborted || current !== generation) return;
        if (protectedQueries.checkOnMount && !onlineManager.isOnline()) throw new Error("Offline membership check");
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
      if (protectedQueries.checkOnMount && event.query.queryKey[0] === membershipKey[0] && event.query.queryKey[1] === space && event.action.type === "invalidate") {
        confirm();
      } else if (event.query.queryKey[0] === "vaults" && event.query.queryKey[1] === "list" && event.action.type === "success") {
        if (state.kind === "denied" || state.kind === "unknown") confirm();
      } else check(event.query);
    });
    for (const query of queries.getAll()) check(query);
    if (protectedQueries.checkOnMount) confirm();
    const recheck = () => { if (protectedQueries.checkOnMount) confirm(); };
    const withholdNavigation = () => {
      if (state.kind !== "checking" && state.kind !== "unknown") return;
      if (window.location.pathname !== withheld.current?.pathname || new URLSearchParams(window.location.search).get("space") !== space) return;
      const safeQuery = `?${new URLSearchParams({ space })}`;
      if (window.location.search !== safeQuery || window.location.hash) {
        withheld.current = { pathname: window.location.pathname, search: window.location.search, hash: window.location.hash };
        setWithheldParams(new URLSearchParams(withheld.current.search));
        window.history.replaceState(null, "", window.location.pathname + safeQuery);
      }
    };
    window.addEventListener("popstate", withholdNavigation);
    window.addEventListener("focus", recheck);
    window.addEventListener("offline", recheck);
    window.addEventListener("online", recheck);
    return () => {
      window.removeEventListener("popstate", withholdNavigation);
      window.removeEventListener("focus", recheck);
      window.removeEventListener("offline", recheck);
      window.removeEventListener("online", recheck);
      unsubscribe();
      clearTimeout(timer);
      controller.abort();
      if (protectedQueries.checkOnMount) void cache.cancelQueries({ queryKey: membershipKey });
    };
  }, [cache, space, protectedQueries]);
  return (
    <ActiveSpace.Provider value={space}>
    <ConfirmedAccess.Provider value={access.kind === "confirmed"}>
    <WithheldParams.Provider value={withheldParams}>
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
    </WithheldParams.Provider>
    </ConfirmedAccess.Provider>
    </ActiveSpace.Provider>
  );
}
