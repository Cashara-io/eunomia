import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ApiError } from "@/lib/api";
import { listMembers } from "@/lib/gen";
import { call } from "@/lib/queries/client";
import ErrorLine from "@/components/ErrorLine";
import { objectMissing, spaceDenied } from "./connection-access";

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
  const [denied, setDenied] = useState(false);
  const [missing, setMissing] = useState<ReadonlySet<string>>(new Set());
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    const queries = cache.getQueryCache();
    const controller = new AbortController();
    const seen = new Set<unknown>();
    let blocked = retry > 0;
    let generation = 0;
    let confirmation: Promise<unknown> | undefined;
    function deny() {
      if (blocked || controller.signal.aborted) return;
      blocked = true;
      generation++;
      const params = new URLSearchParams({ space });
      window.history.replaceState(null, "", `${window.location.pathname}?${params}`);
      setDenied(true);
    }
    function confirm(id?: string) {
      const current = generation;
      // ListMembers permits every member and checks the requested vault, not an entity.
      confirmation ??= call(listMembers({ path: { vault_id: space }, signal: controller.signal }))
        .finally(() => { confirmation = undefined; });
      void confirmation.then(() => {
        if (controller.signal.aborted || current !== generation) return;
        if (blocked) {
          blocked = false;
          setMissing(new Set());
          setDenied(false);
        } else if (id) setMissing(prev => new Set([...prev, id]));
      }).catch(error => {
        if (!controller.signal.aborted && spaceDenied(error, true)) deny();
      });
    }
    function check(query: { queryKey: readonly unknown[]; state: { error: unknown; fetchFailureReason: unknown } }) {
      if (blocked || !query.queryKey.includes(space)) return;
      const objectRead = ["object-profile", "connections"].includes(String(query.queryKey[0]));
      for (const error of [query.state.error, query.state.fetchFailureReason]) {
        if (!error || seen.has(error)) continue;
        seen.add(error);
        if (spaceDenied(error, !objectRead)) deny();
        else if (error instanceof ApiError && error.status === 403) confirm();
        else if (objectRead && objectMissing(error) && typeof query.queryKey[2] === "string") confirm(query.queryKey[2]);
      }
    }
    const unsubscribe = queries.subscribe(event => {
      if (event.type !== "updated") return;
      if (event.query.queryKey[0] === "vaults" && event.query.queryKey[1] === "list" && event.action.type === "success") {
        if (blocked) confirm();
      } else check(event.query);
    });
    for (const query of queries.getAll()) check(query);
    if (retry) confirm();
    return () => {
      unsubscribe();
      controller.abort();
    };
  }, [cache, space, retry]);
  useEffect(() => {
    if (!denied) return;
    const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey.includes(space) };
    void cache.cancelQueries(scoped);
    cache.removeQueries(scoped);
  }, [cache, space, denied]);
  return denied ? (
    <ErrorLine action={<button className="btn btn-sm" onClick={() => setRetry(n => n + 1)}>Retry space access</button>}>
      This information is unavailable to you in this space. Ask a space owner for access.
    </ErrorLine>
  ) : <MissingObjects.Provider value={missing}>{children}</MissingObjects.Provider>;
}
