"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

import { useState } from "react";
import { DownloadSimpleIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import {
  downloadAccessReview,
  useAccessReview,
  useAccessReviews,
  useCompleteAccessReview,
  useDecideAccessReview,
  useStartAccessReview,
} from "@/lib/queries/admin";
import type { AccessReview, AccessReviewSummary } from "@/lib/gen";
import { PanelHead, StatusLine } from "./shared";

const date = (iso?: string | null) => (iso ? new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "—");

const DECISIONS = [
  { id: "keep", label: "Keep" },
  { id: "remove", label: "Remove" },
  { id: "suspend", label: "Suspend" },
] as const;

const WORDS: Record<string, string> = { keep: "Keep", remove: "Remove", suspend: "Suspend" };

/** One person in the snapshot, as the server sent it. */
type SnapshotPerson = {
  user_id: string;
  email: string;
  role: string;
  suspended_at: string | null;
  spaces: { id: string; name: string; role: string; status: string }[];
  tokens: { id: string; name: string }[];
  apps: { id: string; name: string }[];
  sessions: number;
};

function plural(n: number, one: string, many = `${one}s`) {
  return `${n} ${n === 1 ? one : many}`;
}

/* What this person can reach: role, spaces, tokens, apps and sessions, in one dim line. */
function Access({ p }: { p: SnapshotPerson }) {
  const spaces = p.spaces.filter((s) => s.status === "active");
  return (
    <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
      {[
        p.role === "admin" ? "Admin" : "Member",
        p.suspended_at ? "suspended" : null,
        plural(spaces.length, "space"),
        p.tokens.length ? `${plural(p.tokens.length, "API token")} (${p.tokens.map((t) => t.name).join(", ")})` : "no API tokens",
        p.apps.length ? `${plural(p.apps.length, "app")} (${p.apps.map((a) => a.name).join(", ")})` : "no apps",
        plural(p.sessions, "browser session"),
      ]
        .filter(Boolean)
        .join(" · ")}
    </span>
  );
}

function PersonRow({ review, p, onError }: { review: AccessReview; p: SnapshotPerson; onError: (f: Failure | null) => void }) {
  const decide = useDecideAccessReview();
  const current = review.decisions.find((d) => d.user_id === p.user_id);
  const open = review.status === "open";
  const yours = p.user_id === review.you;

  async function choose(decision: string) {
    onError(null);
    try {
      await decide.mutateAsync({ review_id: review.id, user_id: p.user_id, decision });
    } catch (e) {
      onError(failure(e, `Could not record the decision for ${p.email}.`));
    }
  }

  return (
    <li className="py-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between min-w-0" data-testid={`review-person-${p.email}`}>
      <span className="flex flex-col min-w-0">
        <span className="truncate">
          {p.email}
          {yours && (
            <span className="ml-1.5" style={{ color: "var(--ink-faint)" }}>
              (you)
            </span>
          )}
        </span>
        <Access p={p} />
        {current && (
          <span className="text-[12px]" style={{ color: "var(--ink-faint)" }}>
            {WORDS[current.decision]}, decided by {current.reviewer} on {date(current.decided_at)}
          </span>
        )}
      </span>
      {open && yours && !current ? (
        <span className="text-[13px]" style={{ color: "var(--ink-faint)" }}>
          Another admin reviews your access
        </span>
      ) : open && !yours ? (
        <span className="inline-flex gap-1" role="group" aria-label={`Decision for ${p.email}`}>
          {DECISIONS.map((d) => (
            <button
              key={d.id}
              type="button"
              className="pill"
              aria-pressed={current?.decision === d.id}
              disabled={decide.isPending}
              onClick={() => void choose(d.id)}
            >
              {d.label}
            </button>
          ))}
        </span>
      ) : !current ? (
        <span className="text-[13px]" style={{ color: "var(--ink-faint)" }}>
          Not reviewed
        </span>
      ) : null}
    </li>
  );
}

function OpenReview({ id, onError }: { id: string; onError: (f: Failure | null) => void }) {
  const query = useAccessReview(id);
  const complete = useCompleteAccessReview();
  const review = query.data;
  if (query.isError) return <ErrorLine error={failure(query.error, "Could not load the access review.")} />;
  if (!review) return <LoadingSkeleton label="Loading the review" rows={3} />;
  const people = ((review.snapshot as { people?: SnapshotPerson[] }).people ?? []) as SnapshotPerson[];
  const waiting = review.undecided.filter((u) => u !== review.you).length;

  async function finish() {
    onError(null);
    try {
      await complete.mutateAsync(id);
    } catch (e) {
      onError(failure(e, "Could not complete the review."));
    }
  }

  return (
    <section aria-label="Open access review" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <StatusLine tone="warning">
          Open since {date(review.started_at)}, started by {review.started_by}.{" "}
          {waiting ? `${plural(waiting, "person", "people")} still to decide.` : "Everyone has a decision."}
        </StatusLine>
        <button type="button" className="btn btn-sm" disabled={waiting > 0 || complete.isPending} onClick={() => void finish()}>
          {complete.isPending ? "Completing…" : "Complete review"}
        </button>
      </div>
      <ul className="hairline-rows text-[13px]" aria-label="People in this review">
        {people.map((p) => (
          <PersonRow key={p.user_id} review={review} p={p} onError={onError} />
        ))}
      </ul>
      <p className="text-[12px] max-w-[68ch]" style={{ color: "var(--ink-faint)" }}>
        A decision is a record. To suspend someone or take away their access, use People. Once completed, the review can&apos;t be
        changed and is kept in the security log.
      </p>
    </section>
  );
}

function summaryWords(r: AccessReviewSummary) {
  const s = (r.summary ?? {}) as Record<string, number>;
  return [
    s.keep ? `${s.keep} kept` : null,
    s.remove ? `${s.remove} to remove` : null,
    s.suspend ? `${s.suspend} to suspend` : null,
    s.not_reviewed ? `${s.not_reviewed} not reviewed` : null,
  ]
    .filter(Boolean)
    .join(", ");
}

function Past({ reviews, onError }: { reviews: AccessReviewSummary[]; onError: (f: Failure | null) => void }) {
  if (reviews.length === 0) return null;
  return (
    <section aria-labelledby="past-reviews" className="flex flex-col gap-2">
      <h3 id="past-reviews" className="text-[14px] font-medium">
        Completed reviews
      </h3>
      <ul className="hairline-rows text-[13px]">
        {reviews.map((r) => (
          <li key={r.id} className="py-2.5 flex flex-wrap items-center justify-between gap-2 min-w-0" data-testid="completed-review">
            <span className="flex flex-col min-w-0">
              <span>
                {date(r.completed_at)} · {plural(r.people, "person", "people")} · completed by {r.completed_by}
              </span>
              <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                {summaryWords(r)}
              </span>
            </span>
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              onClick={async () => {
                onError(null);
                try {
                  await downloadAccessReview(r.id);
                } catch (e) {
                  onError(failure(e, "Could not download the review."));
                }
              }}
            >
              <DownloadSimpleIcon weight="duotone" size={14} aria-hidden />
              Download
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function AccessReviewsSection() {
  const list = useAccessReviews();
  const start = useStartAccessReview();
  const [error, setError] = useState<Failure | null>(null);
  const reviews = list.data ?? null;
  const open = reviews?.find((r) => r.status === "open");

  async function begin() {
    setError(null);
    try {
      await start.mutateAsync();
    } catch (e) {
      setError(failure(e, "Could not start an access review."));
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <PanelHead
        title="Access reviews"
        action={
          !open && reviews ? (
            <button type="button" className="btn btn-sm" disabled={start.isPending} onClick={() => void begin()}>
              {start.isPending ? "Starting…" : "Start a review"}
            </button>
          ) : undefined
        }
      >
        Check who has access to what: everyone&apos;s role, spaces, API tokens and connected apps. Decide to keep, remove or suspend each
        person, then complete the review for your records.
      </PanelHead>
      {list.isError && <ErrorLine error={failure(list.error, "Could not load access reviews. Reload to try again.")} />}
      {error && <ErrorLine error={error} />}
      {reviews === null ? (
        !list.isError && <LoadingSkeleton label="Loading access reviews" rows={3} />
      ) : (
        <>
          {open ? (
            <OpenReview id={open.id} onError={setError} />
          ) : (
            reviews.length === 0 && (
              <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                No reviews yet.
              </p>
            )
          )}
          <Past reviews={reviews.filter((r) => r.status === "completed")} onError={setError} />
        </>
      )}
    </div>
  );
}
