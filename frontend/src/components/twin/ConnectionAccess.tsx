import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { listMembers } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import ErrorLine from "@/components/ErrorLine";
import { objectMissing, spaceDenied } from "./connection-access";

type Access =
  | { kind: "confirmed" }
  | { kind: "checking"; attempt: number }
  | { kind: "unknown"; attempt: number }
  | { kind: "denied" };

const ConfirmedAccess = createContext(true);
export const useConfirmedConnectionAccess = () => useContext(ConfirmedAccess);
export function ConnectionContent({ children }: { children: ReactNode }) {
  return useConfirmedConnectionAccess() ? children : null;
}

const MissingObjects = createContext<ReadonlySet<string>>(new Set());
export const useMissingConnectionObjects = () => useContext(MissingObjects);

export default function ConnectionAccess({ space, children }: { space: string; children: ReactNode }) {
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
          ["connections", "connection-search"].includes(String(q.queryKey[0])) && q.queryKey[1] === space && q.getObserversCount() === 0 };
        void cache.cancelQueries(scoped);
        cache.removeQueries(scoped);
      });
    };
  }, [cache, space]);
  const [access, setAccess] = useState<Access>({ kind: "confirmed" });
  const [missing, setMissing] = useState<ReadonlySet<string>>(new Set());
  const retry = useRef(() => {});
  useEffect(() => {
    const queries = cache.getQueryCache();
    const controller = new AbortController();
    const seen = new Set<unknown>();
    const pendingMissing = new Set<string>();
    let state: Access = { kind: "confirmed" };
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
      void call(listMembers({ path: { vault_id: space }, signal: controller.signal })).then(() => {
        if (controller.signal.aborted || current !== generation) return;
        const ids = [...pendingMissing];
        pendingMissing.clear();
        setMissing(prev => new Set([...prev, ...ids]));
        transition({ kind: "confirmed" });
      }).catch(error => {
        if (controller.signal.aborted || current !== generation) return;
        if (spaceDenied(error, true)) deny();
        else {
          transition({ kind: "unknown", attempt });
          timer = setTimeout(() => confirm(), Math.min(30_000, 2_000 * 2 ** Math.min(attempt - 1, 4)));
        }
      });
    }
    retry.current = () => confirm();
    function check(query: { queryKey: readonly unknown[]; state: { error: unknown; fetchFailureReason: unknown } }) {
      if (!query.queryKey.includes(space)) return;
      const objectRead = ["object-profile", "connections"].includes(String(query.queryKey[0]));
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
      if (event.query.queryKey[0] === "vaults" && event.query.queryKey[1] === "list" && event.action.type === "success") {
        if (state.kind === "denied" || state.kind === "unknown") confirm();
      } else check(event.query);
    });
    for (const query of queries.getAll()) check(query);
    return () => {
      unsubscribe();
      clearTimeout(timer);
      controller.abort();
    };
  }, [cache, space]);
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
