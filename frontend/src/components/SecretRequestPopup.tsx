"use client";

import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { usePendingSecretRequests } from "@/lib/queries/secretRequests";
import SecretRequestForm from "@/components/SecretRequestForm";

const LATER_KEY = "eunomia.secretRequests.later";

function laterIds(): string[] {
  try {
    return JSON.parse(sessionStorage.getItem(LATER_KEY) ?? "[]");
  } catch {
    return [];
  }
}

/**
 * The secret request pop-up (docs/secret-requests.md): while the person is signed in, pending requests from
 * their agents are polled every 5 seconds (while the tab is visible) and the oldest one they can fill opens
 * as a modal. "Later" hides a request for this tab; the standalone page `/secrets/requests/<id>` shows the
 * same form, so the modal stays out of the way there.
 */
export default function SecretRequestPopup() {
  const pathname = usePathname();
  const onPage = pathname.startsWith("/secrets/requests");
  const pending = usePendingSecretRequests(!onPage);
  // the app shell renders only in the browser (behind AuthGuard), so sessionStorage is there
  const [later, setLater] = useState<string[]>(() => (typeof window === "undefined" ? [] : laterIds()));
  const [done, setDone] = useState<string | null>(null);

  const fillable = (pending.data ?? []).filter((r) => r.can_fill && !later.includes(r.id));
  const current = fillable.length ? fillable[fillable.length - 1] : null; // the list is newest first: the oldest waits longest

  function hide(id: string) {
    const next = [...later, id];
    setLater(next);
    try {
      sessionStorage.setItem(LATER_KEY, JSON.stringify(next));
    } catch {
      /* private mode: hidden for this render only */
    }
  }

  useEffect(() => {
    if (!current) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && hide(current.id);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.id, later]);

  useEffect(() => {
    if (!done) return;
    const t = setTimeout(() => setDone(null), 4000);
    return () => clearTimeout(t);
  }, [done]);

  if (onPage) return null;
  return (
    <>
      {done && (
        <p role="status" className="fixed bottom-4 right-4 z-50 panel px-4 py-2 text-[13px] fade-in" style={{ boxShadow: "var(--shadow-pop)" }}>
          {done}
        </p>
      )}
      {current && (
        <div className="fixed inset-0 z-50 flex items-start justify-center px-4 pt-[12vh]" style={{ background: "var(--scrim)" }}>
          <div
            role="dialog"
            aria-modal="true"
            aria-label="An agent is asking for a secret"
            data-testid="secret-request-popup"
            className="w-full max-w-[520px] panel p-5 pop-in max-h-[80vh] overflow-y-auto"
            style={{ boxShadow: "var(--shadow-pop)" }}
          >
            <div className="label mb-3">An agent is asking for a secret{fillable.length > 1 ? ` (1 of ${fillable.length})` : ""}</div>
            <SecretRequestForm
              key={current.id}
              request={current}
              onLater={() => hide(current.id)}
              onDone={(outcome) => {
                setDone(outcome === "filled" ? "Saved. The agent can now use it without seeing it." : "Declined. The agent has been told.");
                pending.refetch();
              }}
            />
          </div>
        </div>
      )}
    </>
  );
}
