"use client";

import { useEffect, useState } from "react";
import { useRouter, usePathname } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { bootstrapQuery, meQuery } from "@/lib/queries/auth";
import { isSessionEnded } from "@/lib/queries/client";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";

/** Gates every page under the `(app)` route group behind a session check.
 * No session (401 from `/api/auth/me`; other failures show an error with retry) sends a fresh visitor to `/register`
 * if no account exists yet anywhere on the instance, else `/login`.
 * An existing-but-unonboarded session is sent to `/onboarding` first. */
export default function AuthGuard({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const pathname = usePathname();
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<Failure | null>(null);
  const [attempt, setAttempt] = useState(0);
  const queryClient = useQueryClient();

  useEffect(() => {
    let cancelled = false;
    queryClient
      .fetchQuery(meQuery())
      .then((me) => {
        if (cancelled) return;
        if (!me.onboarded) {
          router.replace(`/onboarding?next=${encodeURIComponent(pathname + window.location.search)}`);
          return;
        }
        setReady(true);
      })
      .catch(async (e) => {
        if (cancelled) return;
        if (!isSessionEnded(e)) {
          setError(failure(e, "Could not check your sign-in. Eunomia may be restarting."));
          return;
        }
        const next = encodeURIComponent(pathname);
        const { has_users } = await queryClient.fetchQuery(bootstrapQuery()).catch(() => ({ has_users: true }));
        router.replace(has_users ? `/login?next=${next}` : "/register");
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pathname, attempt]);

  if (error)
    return (
      <div className="p-6 flex flex-col items-start gap-3 max-w-xl">
        <ErrorLine error={error} />
        <button type="button" className="btn" onClick={() => {
            setError(null);
            setAttempt((n) => n + 1);
          }}>
          Try again
        </button>
      </div>
    );
  if (!ready) return null;
  return <>{children}</>;
}
