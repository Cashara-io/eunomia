import { useEffect, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ApiError } from "@/lib/api";
import ReadState from "./ReadState";

function accessDenied(error: unknown): error is ApiError {
  return error instanceof ApiError &&
    ([403, 404].includes(error.status) || /denied|forbidden|not[_ .-]?(?:a[_ .-]?)?member/i.test(error.code));
}

export default function ConnectionAccess({ space, children }: { space: string; children: ReactNode }) {
  const cache = useQueryClient();
  const [denied, setDenied] = useState<ApiError | null>(null);
  useEffect(() => {
    const queries = cache.getQueryCache();
    function check() {
      for (const query of queries.getAll()) {
        if (!query.queryKey.includes(space)) continue;
        const error = [query.state.error, query.state.fetchFailureReason].find(accessDenied);
        if (!error) continue;
        // A retrying read has a failure reason before its final error is set.
        const denied = new ApiError(403, error.code, "Space unavailable");
        setDenied(current => current ?? denied);
        break;
      }
    }
    const unsubscribe = queries.subscribe(check);
    check();
    return unsubscribe;
  }, [cache, space]);
  useEffect(() => {
    if (!denied) return;
    const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey.includes(space) };
    // Children have unmounted. Cancel first so a late response cannot restore names.
    void cache.cancelQueries(scoped).then(() => cache.removeQueries(scoped));
  }, [cache, space, denied]);
  return denied ? <ReadState error={denied} retry={() => {}} /> : children;
}
