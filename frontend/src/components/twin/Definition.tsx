"use client";

import { useEffect, useEffectEvent, useId, useRef, useState } from "react";
import Link from "next/link";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { CheckIcon, ShieldCheckIcon, XIcon } from "@phosphor-icons/react/ssr";
import { API_URL, ApiError } from "@/lib/api";
import { haptic } from "@/lib/haptics";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import {
  useDecideKind,
  useDecideLink,
  useDecideMetric,
  useDecideProperty,
  useTryMetric,
  type OntologyOut,
} from "@/lib/queries/ontology";
import { MetricRow, STATUS, words } from "./DefinitionRows";
import {
  definitions,
  twinHref,
  type Definition as DefinitionItem,
} from "./model";
import { isTyping, mergeTargets, reviewFields } from "./review-model";
import styles from "./review.module.css";
import ReviewEvidence from "./ReviewEvidence";
import MergePreview from "./MergePreview";

const activitySchema = z.object({
  events: z.array(
    z.object({
      action: z.string(),
      revision: z.number(),
      actor_name: z.string(),
      at: z.string(),
      reason: z.string().nullable(),
    }),
  ),
});
type Action =
  | { action: "approve" }
  | { action: "reject"; merge_into?: string }
  | { action: "describe"; description: string };
type SaveState =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "failed"; error: Failure; action: Action }
  | { kind: "stale"; refreshFailed: boolean }
  | {
      kind: "saved";
      definition: DefinitionItem;
      action: Action;
      reason: string;
    };

export default function Definition({
  definition: incoming,
  data,
  isOwner,
  locked = false,
  initialReason = "",
  onNext,
  onBusyChange,
  queueReview = false,
}: {
  definition: DefinitionItem;
  data: OntologyOut;
  isOwner: boolean;
  locked?: boolean;
  initialReason?: string;
  queueReview?: boolean;
  onNext?: () => void;
  onBusyChange?: (busy: boolean) => void;
}) {
  const cache = useQueryClient();
  const [d, setDefinition] = useState(incoming);
  const [reason, setReason] = useState(initialReason);
  const [targetId, setTargetId] = useState("");
  const [description, setDescription] = useState<string | null>(null);
  const editButton = useRef<HTMLButtonElement>(null);
  const [state, setState] = useState<SaveState>({ kind: "idle" });
  const inFlight = useRef(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const mergeSelect = useRef<HTMLSelectElement>(null);
  const receiptHeading = useRef<HTMLHeadingElement>(null);
  const reasonId = useId();
  const descriptionId = useId();
  const kind = useDecideKind(data.vault);
  const link = useDecideLink(data.vault);
  const property = useDecideProperty(data.vault);
  const metric = useDecideMetric(data.vault);
  const preview = useTryMetric(data.vault);
  const history = useQuery({
    queryKey: ["ontology-activity", data.vault, "decision-receipt", d.id],
    queryFn: async () => {
      const params = new URLSearchParams({
        definition_type: d.type,
        kind: d.kind,
        name: d.name,
        newest: "true",
        limit: "30",
      });
      const response = await fetch(
        `${API_URL}/api/vaults/${encodeURIComponent(data.vault)}/ontology/activity?${params}`,
        { credentials: "include" },
      );
      if (!response.ok) throw new Error("Activity is unavailable");
      return activitySchema.parse(await response.json());
    },
  });
  const proposer = history.data?.events.find(
    (e) => e.action === "proposed" && e.revision === d.item.revision,
  );
  const targets = mergeTargets(d, data);
  const target = targets.find((t) => t.id === targetId);
  const stale =
    state.kind === "stale" ||
    (state.kind !== "saved" && incoming.item.revision !== d.item.revision);
  const busy = locked || state.kind === "saving";
  const canDecide = isOwner && !busy && !stale && state.kind !== "saved";
  const builtIn = d.type === "kind" && d.item.built_in;

  useEffect(() => {
    if (queueReview) heading.current?.focus({ preventScroll: true });
  }, [queueReview]);
  useEffect(() => {
    if (state.kind === "saved") receiptHeading.current?.focus();
  }, [state]);

  async function refreshLatest() {
    try {
      await cache.refetchQueries(
        { queryKey: ["ontology", data.vault], exact: true },
        { throwOnError: true },
      );
      setState({ kind: "stale", refreshFailed: false });
    } catch {
      setState({ kind: "stale", refreshFailed: true });
    }
  }
  async function decide(action: Action) {
    if (!canDecide || inFlight.current) return;
    inFlight.current = true;
    onBusyChange?.(true);
    setState({ kind: "saving" });
    const common = { decision: action, revision: d.item.revision, reason };
    try {
      const saved =
        d.type === "kind"
          ? await kind.mutateAsync({ ...common, kind: d.kind })
          : d.type === "property"
            ? await property.mutateAsync({
                ...common,
                kind: d.kind,
                name: d.name,
              })
            : d.type === "metric"
              ? await metric.mutateAsync({
                  ...common,
                  kind: d.kind,
                  name: d.name,
                })
              : await link.mutateAsync({
                  ...common,
                  from_kind: d.item.from_kind,
                  label: d.item.label,
                  to_kind: d.item.to_kind,
                });
      const updated = definitions(saved).find((item) => item.id === d.id);
      if (!updated)
        throw new Error(
          "The server response did not include this definition. Refresh before deciding again.",
        );
      setState({ kind: "saved", definition: updated, action, reason });
      haptic("success");
    } catch (e) {
      if (e instanceof ApiError && e.code === "ontology.stale_revision") {
        setState({ kind: "stale", refreshFailed: false });
        await refreshLatest();
      } else
        setState({
          kind: "failed",
          action,
          error: failure(
            e,
            "The decision was not confirmed. Your reason and evidence are kept here.",
          ),
        });
    } finally {
      inFlight.current = false;
      onBusyChange?.(false);
    }
  }
  const shortcut = useEffectEvent((e: KeyboardEvent) => {
    if (
      e.defaultPrevented ||
      e.metaKey ||
      e.ctrlKey ||
      e.altKey ||
      document.querySelector("dialog[open]")
    )
      return;
    if (e.key === "Escape" && targetId && !busy) {
      e.preventDefault();
      e.stopImmediatePropagation();
      setTargetId("");
      mergeSelect.current?.focus();
      return;
    }
    if (e.key === "Escape" && description !== null && !busy) {
      e.preventDefault();
      e.stopImmediatePropagation();
      setDescription(null);
      editButton.current?.focus();
      return;
    }
    if (isTyping(e.target) || !canDecide || targetId || description !== null)
      return;
    if (e.key.toLowerCase() === "a") {
      e.preventDefault();
      void decide({ action: "approve" });
    }
    if (e.key.toLowerCase() === "r" && !builtIn) {
      e.preventDefault();
      void decide({ action: "reject" });
    }
  });
  useEffect(() => {
    if (!queueReview) return;
    const handler = (e: KeyboardEvent) => shortcut(e);
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [queueReview]);

  const receiptEvent =
    state.kind === "saved"
      ? history.data?.events.find(
          (e) =>
            e.revision === state.definition.item.revision &&
            ["approved", "rejected", "merged"].includes(e.action),
        )
      : undefined;
  const evidence = <ReviewEvidence definition={d} data={data} />;
  const displayed = state.kind === "saved" ? state.definition : d;

  return (
    <article className={styles.reading} aria-busy={busy}>
      <header className={styles.identity}>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h3 ref={heading} tabIndex={-1}>
            {words(d.name)}
          </h3>
          <span
            className={styles.suggested}
            data-saved={state.kind === "saved"}
          >
            {displayed.item.status === "proposed"
              ? "Suggested"
              : STATUS[displayed.item.status].label}
          </span>
        </div>
        <div className={styles.meta}>
          <span>
            {words(d.type)}
            {d.type !== "kind" && ` on ${words(d.kind)}`}
          </span>
          <span>
            {proposer
              ? `Proposed by ${proposer.actor_name}`
              : d.item.origin === "agent"
                ? "Proposed by an AI app, name unavailable"
                : d.item.origin === "human"
                  ? "Added by a person, name unavailable"
                  : d.item.origin === "suggested"
                    ? "Suggested by the server model"
                    : "Proposer not recorded"}
          </span>
          <span>Version {displayed.item.revision}</span>
        </div>
      </header>
      {state.kind === "saved" ? (
        <>
          <section
            className={`${styles.receipt} ${styles.reveal}`}
            aria-label="Saved decision"
          >
            <span className={styles.receiptIcon}>
              <CheckIcon size={24} weight="duotone" />
            </span>
            <h4 ref={receiptHeading} tabIndex={-1}>
              Saved. {words(d.name).replace(/^./, (c) => c.toUpperCase())}{" "}
              {state.action.action === "describe"
                ? "description updated"
                : state.action.action === "approve"
                  ? "approved"
                  : state.action.merge_into
                    ? `merged into ${words(state.action.merge_into)}`
                    : "rejected"}
              .
            </h4>
            <p className={styles.dim}>The server confirmed this decision.</p>
            <dl>
              <dt>Definition</dt>
              <dd>
                {words(d.name)}, version {state.definition.item.revision}
              </dd>
              <dt>Reviewed version</dt>
              <dd>{d.item.revision}</dd>
              <dt>Decision</dt>
              <dd>
                {state.action.action === "describe"
                  ? "Edit description"
                  : state.action.action === "approve"
                    ? "Approve"
                    : state.action.merge_into
                      ? "Reject and redirect"
                      : "Reject without merging"}
              </dd>
              <dt>Reason</dt>
              <dd>{state.reason || "No reason supplied"}</dd>
              <dt>Recorded by</dt>
              <dd>
                {receiptEvent
                  ? `${receiptEvent.actor_name} · ${new Date(receiptEvent.at).toLocaleString("en-AU")}`
                  : "Actor and time are awaiting the activity record."}
              </dd>
            </dl>
            <div className="flex flex-wrap gap-3">
              <Link
                className="btn btn-sm"
                href={twinHref("/ontology/activity", data.vault, {
                  definition_type: d.type,
                  kind: d.kind,
                })}
              >
                View Activity
              </Link>
              {onNext && (
                <button className="btn btn-primary btn-sm" onClick={onNext}>
                  Next suggestion
                </button>
              )}
            </div>
          </section>
          <details className={styles.section}>
            <summary className="cursor-pointer min-h-11">
              Review the evidence again
            </summary>
            {evidence}
          </details>
        </>
      ) : (
        <>
          {stale && (
            <section role="alert" className={styles.notice}>
              <h4 className="font-medium mb-2">This definition changed</h4>
              <RevisionChanges before={d} after={incoming} />
              <p className="my-3">
                Your reason is kept. Review the latest fields before deciding
                again.
              </p>
              {state.kind === "stale" && state.refreshFailed ? (
                <button
                  className="btn btn-sm"
                  onClick={() => void refreshLatest()}
                >
                  Retry loading latest version
                </button>
              ) : incoming.item.revision !== d.item.revision ? (
                <button
                  className="btn btn-sm"
                  onClick={() => {
                    setDefinition(incoming);
                    setTargetId("");
                    setState({ kind: "idle" });
                    heading.current?.focus();
                  }}
                >
                  Review latest version
                </button>
              ) : (
                <p>
                  The latest version is not available yet.{" "}
                  <button
                    className="btn btn-sm"
                    onClick={() => void refreshLatest()}
                  >
                    Retry loading latest version
                  </button>
                </p>
              )}
            </section>
          )}
          {evidence}
          {d.type === "metric" && (
            <details className={styles.section}>
              <summary className="cursor-pointer min-h-11">
                Try calculation
              </summary>
              <ul>
                <MetricRow
                  item={d.item}
                  isOwner={false}
                  busy={busy}
                  targets={[]}
                  onDecide={async () => false}
                  onTry={() =>
                    preview.mutateAsync({ kind: d.kind, name: d.name })
                  }
                />
              </ul>
            </details>
          )}
          {isOwner && (
            <details
              className={styles.section}
              aria-label="Other definition options"
            >
              <summary className="cursor-pointer min-h-11 py-2">
                {targets.length > 0 && !builtIn
                  ? "Merge or edit definition"
                  : "Edit definition"}
              </summary>
              {isOwner && (
                <section className={styles.section}>
                  {description === null ? (
                    <button
                      ref={editButton}
                      className="btn btn-ghost btn-sm"
                      disabled={!canDecide}
                      onClick={() => {
                        setTargetId("");
                        setDescription(d.item.description ?? "");
                      }}
                    >
                      Edit description
                    </button>
                  ) : (
                    <div className={styles.reason}>
                      <label htmlFor={descriptionId}>Description</label>
                      <textarea
                        id={descriptionId}
                        className="field"
                        autoFocus
                        value={description}
                        maxLength={500}
                        disabled={busy || stale}
                        onChange={(e) => setDescription(e.target.value)}
                      />
                    </div>
                  )}
                </section>
              )}
              {isOwner &&
                description === null &&
                !builtIn &&
                targets.length > 0 && (
                  <MergePreview
                    definition={d}
                    targets={targets}
                    targetId={targetId}
                    selectRef={mergeSelect}
                    disabled={busy || stale}
                    onTargetChange={(id) => {
                      setTargetId(id);
                      setState({ kind: "idle" });
                    }}
                  />
                )}
            </details>
          )}
          {isOwner ? (
            <>
              <section className={styles.section}>
                <label className={styles.reason} htmlFor={reasonId}>
                  Your reason, saved with the decision (optional)
                  <textarea
                    id={reasonId}
                    className="field"
                    maxLength={500}
                    value={reason}
                    onChange={(e) => setReason(e.target.value)}
                    disabled={busy}
                    placeholder="What supports your decision?"
                  />
                </label>
              </section>
              {state.kind === "failed" && (
                <div className={styles.notice}>
                  <ErrorLine error={state.error} />
                  <p className="my-2">
                    Your reason and evidence are kept here. The save has not
                    been confirmed.
                  </p>
                  <button
                    className="btn btn-sm"
                    disabled={!canDecide}
                    onClick={() => void decide(state.action)}
                  >
                    Retry
                  </button>
                </div>
              )}
              <footer className={styles.actions}>
                <span className={styles.shortcuts}>
                  {busy ? (
                    "Saving decision…"
                  ) : description !== null ? (
                    <>
                      <kbd>Esc</kbd> Cancel edit
                    </>
                  ) : target ? (
                    <>
                      <kbd>Esc</kbd> Cancel merge
                    </>
                  ) : (
                    <>
                      <kbd>A</kbd> Approve <kbd>R</kbd> Reject
                    </>
                  )}
                </span>
                <div className="flex flex-wrap gap-2">
                  {description !== null ? (
                    <>
                      <button
                        className="btn btn-sm"
                        disabled={busy}
                        onClick={() => {
                          setDescription(null);
                          editButton.current?.focus();
                        }}
                      >
                        Cancel edit
                      </button>
                      <button
                        className="btn btn-primary btn-sm"
                        disabled={!canDecide}
                        onClick={() =>
                          void decide({
                            action: "describe",
                            description: description.trim(),
                          })
                        }
                      >
                        Save description
                      </button>
                    </>
                  ) : target ? (
                    <>
                      <button
                        className="btn btn-sm"
                        disabled={busy}
                        onClick={() => {
                          setTargetId("");
                          mergeSelect.current?.focus();
                        }}
                      >
                        Cancel merge
                      </button>
                      <button
                        className="btn btn-primary btn-sm"
                        disabled={!canDecide}
                        onClick={() =>
                          void decide({
                            action: "reject",
                            merge_into: target.name,
                          })
                        }
                      >
                        Confirm merge
                      </button>
                    </>
                  ) : (
                    <>
                      {!builtIn && (
                        <button
                          className="btn btn-sm"
                          disabled={!canDecide}
                          onClick={() => void decide({ action: "reject" })}
                        >
                          <XIcon size={16} />
                          Reject
                        </button>
                      )}
                      <button
                        className="btn btn-primary btn-sm"
                        disabled={!canDecide}
                        onClick={() => void decide({ action: "approve" })}
                      >
                        <CheckIcon size={16} />
                        Approve {words(d.name)}
                      </button>
                    </>
                  )}
                </div>
              </footer>
            </>
          ) : (
            <section className={styles.section}>
              <h4>
                <ShieldCheckIcon size={20} weight="duotone" />
                Owner-only decision
              </h4>
              <p className={styles.dim}>
                You can read this evidence. Only an owner of this space can
                approve, reject or merge a definition.
              </p>
            </section>
          )}
        </>
      )}
      <p role="status" className="sr-only">
        {state.kind === "saving"
          ? "Saving decision"
          : state.kind === "saved"
            ? "Decision saved by the server"
            : ""}
      </p>
    </article>
  );
}

export function RevisionChanges({
  before,
  after,
}: {
  before: DefinitionItem;
  after: DefinitionItem;
}) {
  const previous = reviewFields(before);
  return (
    <dl className="text-[13px] flex flex-col gap-3">
      {Object.entries(reviewFields(after))
        .filter(([key, value]) => previous[key] !== value)
        .map(([key, value]) => (
          <div key={key}>
            <dt className="font-medium">{key}</dt>
            <dd>Before: {previous[key] || "None"}</dd>
            <dd>Now: {value || "None"}</dd>
          </div>
        ))}
    </dl>
  );
}
