"use client";

import {
  useEffect, useEffectEvent, useId, useImperativeHandle, useRef, useState,
  type Ref,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { CheckIcon, ShieldCheckIcon, XIcon } from "@phosphor-icons/react/ssr";
import { ApiError } from "@/lib/api";
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
import { MetricRow, STATUS } from "./DefinitionRows";
import {
  definitions,
  twinHref,
  type Definition as DefinitionItem,
} from "./model";
import {
  hasReviewOverlay, ignoreReviewKey, mergeTargets, reviewFields,
  savedDecision, supportsMerge, type ReviewOperation,
} from "./review-model";
import styles from "./review.module.css";
import { useReviewArming } from "./use-review-arming";
import ReviewEvidence from "./ReviewEvidence";
import MergePreview from "./MergePreview";
import { humanLabel, ReviewTime } from "./review-format";
import { useReviewActivity } from "./review-activity";
import { useLandscapeTint } from "./landscape-shared";
import { ReviewTypeIcon } from "./ReviewInboxMeta";
import { isReviewOwner, ownerOnly } from "./review-access";

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
      operation: ReviewOperation;
      result: ReturnType<typeof savedDecision>;
      definition: DefinitionItem;
      reason: string;
    };

export type ReviewHandle = { beginNavigation: () => void };

export default function Definition({
  ref,
  definition: incoming,
  data,
  isOwner,
  locked = false,
  initialReason = "",
  onNext,
  onBusyChange,
  queueReview = false,
}: {
  ref?: Ref<ReviewHandle>;
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
  const reviewRegion = useRef<HTMLElement>(null);
  const approveButton = useRef<HTMLButtonElement>(null);
  const rejectButton = useRef<HTMLButtonElement>(null);
  const [ownerDenied, setOwnerDenied] = useState(false);
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
  const history = useReviewActivity(data.vault, d);
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
  const arming = useReviewArming({
    id: d.id, revision: d.item.revision,
    incomingId: incoming.id, incomingRevision: incoming.item.revision,
    enabled: canDecide, mode: description !== null ? "describe" : targetId ? "merge" : "decision",
    region: reviewRegion, heading,
  });
  useImperativeHandle(ref, () => ({ beginNavigation: arming.beginNavigation }));
  const { tint } = useLandscapeTint();
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
  async function decide(
    action: Action,
  ) {
    if (!isOwner || !isReviewOwner(cache, data.vault)) {
      arming.disarm();
      setOwnerDenied(true);
      return;
    }
    if (action.action === "reject" && action.merge_into && !supportsMerge(d)) return;
    const latest = cache.getQueryData<OntologyOut>(["ontology", data.vault]);
    const latestItem = latest && definitions(latest).find(item => item.id === d.id);
    if (!arming.available() || !latestItem || latestItem.item.revision !== d.item.revision) return;
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
      const operation = action.action === "reject" && action.merge_into ? "merge" : action.action;
      const result = savedDecision(operation, updated, d.item.revision,
        action.action === "reject" ? action.merge_into : undefined);
      setState({ kind: "saved", definition: updated, reason, operation, result });
      haptic(result.confirmed ? "success" : "error");
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
  function decisionKeyDown(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    action: Action,
  ) {
    if (event.key !== "Enter" && event.key !== " ") return;
    const ignored = ignoreReviewKey(event.nativeEvent, reviewRegion.current) || hasReviewOverlay();
    event.preventDefault();
    if (ignored) return;
    if (arming.hasShortcut() && !arming.consume()) return;
    void decide(action);
  }
  const shortcut = useEffectEvent((e: KeyboardEvent) => {
    if (ignoreReviewKey(e, reviewRegion.current, true)) return;
    if (e.key === "Escape") arming.disarm();
    if (e.key === "Escape" && targetId && !busy && !hasReviewOverlay(false)) {
      e.preventDefault();
      e.stopImmediatePropagation();
      setTargetId("");
      mergeSelect.current?.focus();
      return;
    }
    if (hasReviewOverlay()) return;
    if (e.key === "Escape" && description !== null && !busy) {
      e.preventDefault();
      e.stopImmediatePropagation();
      setDescription(null);
      editButton.current?.focus();
      return;
    }
    if (!canDecide || targetId || description !== null) return;
    if (e.key === "a" || (e.key === "r" && !builtIn)) {
      e.preventDefault();
      (e.key === "a" ? approveButton : rejectButton).current?.focus();
      arming.arm();
    }
  });
  useEffect(() => {
    if (!queueReview) return;
    const handler = (e: KeyboardEvent) => shortcut(e);
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [queueReview]);

  const receiptEvent =
    state.kind === "saved" && state.result.confirmed
      ? history.data?.events.find(
          (e) =>
            e.revision === state.definition.item.revision &&
            e.action === ({ approve: "approved", reject: "rejected", merge: "merged", describe: "described" }[state.operation]),
        )
      : undefined;
  const evidence = <ReviewEvidence definition={d} data={data} />;
  const displayed = state.kind === "saved" ? state.definition : d;

  return (
    <article
      ref={reviewRegion} className={styles.reading} aria-busy={busy}
      onKeyDownCapture={event => {
        if (event.key === "Escape") arming.disarm();
      }}
      onBlurCapture={event => {
        if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) arming.disarm();
      }}
    >
      <header className={styles.identity} style={tint(d.kind)}>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <span className={styles.identityIcon}><ReviewTypeIcon definition={d} /></span>
          <h3 ref={heading} tabIndex={-1}>
            {humanLabel(d.name)}
          </h3>
          <span
            className={styles.suggested}
            data-saved={state.kind === "saved" && state.result.confirmed}
          >
            {displayed.item.status === "proposed"
              ? "Suggested"
              : STATUS[displayed.item.status].label}
          </span>
        </div>
        <div className={styles.meta}>
          <span>
            {humanLabel(d.type)}
            {d.type !== "kind" && ` on ${humanLabel(d.kind)}`}
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
            aria-label={state.result.confirmed ? "Saved decision" : "Decision not confirmed"}
          >
            <span className={styles.receiptIcon}>
              {state.result.confirmed
                ? <CheckIcon size={24} weight="duotone" />
                : <XIcon size={24} weight="duotone" />}
            </span>
            <h4 ref={receiptHeading} tabIndex={-1}>
              {state.result.confirmed
                ? `Saved. ${humanLabel(d.name)} ${state.result.sentence}.`
                : "Decision not confirmed."}
            </h4>
            <p className={styles.dim}>
              {state.result.confirmed ? "The server confirmed this decision." : state.result.message}
            </p>
            <dl>
              <dt>Definition</dt>
              <dd>
                {humanLabel(d.name)}, version {state.definition.item.revision}
              </dd>
              <dt>Reviewed version</dt>
              <dd>{d.item.revision}</dd>
              <dt>{state.result.confirmed ? "Decision" : "Returned state"}</dt>
              <dd>
                {state.result.label}
              </dd>
              <dt>{state.result.confirmed ? "Reason" : "Submitted reason"}</dt>
              <dd>{state.reason || "No reason supplied"}</dd>
              {state.result.confirmed && <>
                <dt>Recorded by</dt>
                <dd>
                  {receiptEvent
                    ? <>{receiptEvent.actor_name} · <ReviewTime at={receiptEvent.at} /></>
                    : "Actor and time are awaiting the activity record."}
                </dd>
              </>}
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
          {ownerDenied && <p role="alert" className={styles.notice}>{ownerOnly}</p>}
          {evidence}
          {!supportsMerge(d) && <p className={`${styles.dim} px-6 pb-4`}>Merging is not available for links.</p>}
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
                        arming.disarm();
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
                      arming.disarm();
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
                    data-review-decision
                    onClick={() => void decide(state.action)}
                  >
                    Retry
                  </button>
                </div>
              )}
              <footer className={styles.actions}>
                <span className={styles.armedHint}>Keyboard decision ready. Press <kbd>Enter</kbd> to confirm, or <kbd>Esc</kbd> to cancel.</span>
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
                      <kbd>A</kbd> Focus Approve <kbd>R</kbd> Focus Reject <kbd>Enter</kbd> Confirm
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
                        data-review-decision
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
                        data-review-decision
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
                          ref={rejectButton}
                          data-review-decision
                          onKeyDown={(event) => decisionKeyDown(event, { action: "reject" })}
                          onClick={() => void decide({ action: "reject" })}
                        >
                          <XIcon size={16} />
                          Reject
                        </button>
                      )}
                      <button
                        className="btn btn-primary btn-sm"
                        disabled={!canDecide}
                        ref={approveButton}
                        data-review-decision
                        onKeyDown={(event) => decisionKeyDown(event, { action: "approve" })}
                        onClick={() => void decide({ action: "approve" })}
                      >
                        <CheckIcon size={16} />
                        Approve {humanLabel(d.name)}
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
            ? state.result.confirmed ? "Decision saved by the server" : "Decision not confirmed by the server"
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
    <dl className="text-detail flex flex-col gap-3">
      {Object.entries(reviewFields(after))
        .filter(([key, value]) => previous[key] !== value)
        .map(([key, value]) => (
          <div key={key}>
            <dt className="font-medium">{key}</dt>
            <dd className={styles.removed}>Before: <del>{previous[key] || "None"}</del></dd>
            <dd className={styles.added}>Now: <ins>{value || "None"}</ins></dd>
          </div>
        ))}
    </dl>
  );
}
