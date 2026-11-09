"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

// A space's kinds and links: what its memories are about (person, customer, decision, ...) and how
// they connect (works at, owns, ...). AI apps and the server's model only suggest; an owner decides.
import { useState, type ReactNode } from "react";
import {
  ArrowsMergeIcon,
  CheckIcon,
  PencilSimpleIcon,
  PlayIcon,
  XIcon,
} from "@phosphor-icons/react/ssr";
import {
  asOfDate,
  MetricNumber,
  MetricWarnings,
} from "@/components/chat/views/MetricView";
import type { MetricRun } from "@/components/chat/views/types";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import Duo from "@/components/ui/Duo";
import StatusLabel, { type StatusTone } from "@/components/ui/StatusLabel";
import {
  useReadiness,
  useSetStrict,
  type KindItem,
  type LinkItem,
  type MetricItem,
  type PropertyItem,
  type ViewStatus,
} from "@/lib/queries/ontology";
import type { MetricDecision, PropertyDecision } from "@/lib/gen";
import { MatchShare } from "@/components/vaults/AutomapBar";

export const STATUS: Record<ViewStatus, { tone: StatusTone; label: string }> = {
  observed: { tone: "neutral", label: "In use" },
  proposed: { tone: "info", label: "Suggested" },
  approved: { tone: "good", label: "Approved" },
  rejected: { tone: "warning", label: "Rejected" },
};

export const words = (name: string) => name.replace(/_/g, " ");

const counted = (n: number, one: string, many: string) =>
  `${n} ${n === 1 ? one : many}`;

/** Whether the space can answer number questions yet, and what still stands in the way. */
export function ReadinessPanel({ vaultId }: { vaultId: string }) {
  const query = useReadiness(vaultId);
  const r = query.data;
  if (query.isError)
    return (
      <ErrorLine
        error={failure(
          query.error,
          "Could not check whether this space is ready.",
        )}
      />
    );
  if (!r) return <LoadingSkeleton label="Loading readiness" rows={1} />;
  const approved = [
    counted(r.kinds.approved, "kind", "kinds"),
    counted(r.properties.approved, "property", "properties"),
    counted(r.metrics.approved, "metric", "metrics"),
    counted(r.links.approved, "link", "links"),
  ].join(", ");
  return (
    <section
      aria-label="Ready for numbers"
      className="flex flex-col gap-1.5 py-1"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <StatusLabel tone={r.ready ? "good" : "warning"}>
          {r.ready
            ? "Ready to answer numbers"
            : "Not ready to answer every number yet"}
        </StatusLabel>
        <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          Approved: {approved}
        </span>
      </div>
      <p className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
        Ready means nothing proposed is waiting for you, at least one metric is
        approved, and every property an approved metric reads is approved and
        has values. A kind with no metric, like a person or a supplier, never
        holds it up.
      </p>
      {r.blockers.length > 0 && (
        <ul
          className="flex flex-col gap-0.5 text-[13px] list-disc pl-5"
          aria-label="What is in the way"
        >
          {r.blockers.slice(0, 5).map((b) => (
            <li key={b}>{b}</li>
          ))}
          {r.blockers.length > 5 && (
            <li style={{ color: "var(--ink-dim)" }}>
              and {r.blockers.length - 5} more
            </li>
          )}
        </ul>
      )}
    </section>
  );
}

/** "Only approved kinds and links": an owner's switch; members see whether it is on. */
export function StrictSwitch({
  vaultId,
  on,
  isOwner,
  onError,
}: {
  vaultId: string;
  on: boolean;
  isOwner: boolean;
  onError: (f: Failure | null) => void;
}) {
  const setStrict = useSetStrict(vaultId);
  async function toggle(next: boolean) {
    onError(null);
    try {
      await setStrict.mutateAsync(next);
    } catch (e) {
      onError(failure(e, "Could not change that. Try again."));
    }
  }
  return (
    <label
      htmlFor={`strict-${vaultId}`}
      className={`flex items-start gap-3 max-w-[68ch] ${isOwner ? "cursor-pointer" : ""}`}
    >
      <input
        id={`strict-${vaultId}`}
        type="checkbox"
        role="switch"
        className="mt-1"
        style={{ accentColor: "var(--accent)" }}
        checked={on}
        disabled={!isOwner || setStrict.isPending}
        onChange={(e) => toggle(e.target.checked)}
      />
      <span className="flex flex-col gap-0.5">
        <span className="text-[14px] font-medium">
          Only approved kinds and links
        </span>
        <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {on
            ? "On. AI apps can only save things under the kinds and links approved here. Anything else is turned away and appears in Review as a suggestion."
            : "Off. AI apps can save things under any kind or link; new ones appear in Structure as in use. Turn this on to accept only what you approved."}
          {!isOwner && " Only an owner of this space can change it."}
        </span>
      </span>
    </label>
  );
}

type Decide<D> = (decision: D, fallback: string) => Promise<boolean>;

export function KindRow({
  item,
  isOwner,
  busy,
  targets,
  onDecide,
  children,
}: {
  item: KindItem;
  isOwner: boolean;
  busy: boolean;
  targets: { value: string; label: string }[];
  onDecide: Decide<
    | { action: "approve" }
    | { action: "reject"; merge_into?: string | null }
    | { action: "describe"; description: string }
  >;
  children?: ReactNode;
}) {
  const [editing, setEditing] = useState(false);
  const count = `${item.entities} ${item.entities === 1 ? "entry" : "entries"}`;
  const suggestedMerge =
    item.status === "proposed" && item.merge_into ? item.merge_into : null;
  const detail =
    item.status === "rejected"
      ? item.merge_into
        ? `New memories go to ${words(item.merge_into)}.`
        : "New memories of this kind are refused."
      : suggestedMerge
        ? `Suggested: merge into ${words(suggestedMerge)}.`
        : null;
  return (
    <li className="flex flex-col gap-1.5 py-2.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <span className="flex-1 min-w-[160px] flex items-baseline gap-2">
          <span className="text-[14px] font-medium">{words(item.kind)}</span>
          <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {item.built_in ? `built in · ${count}` : count}
          </span>
        </span>
        <StatusLabel tone={STATUS[item.status].tone}>
          {STATUS[item.status].label}
        </StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={item.samples}
        editing={editing}
        busy={busy}
        onSave={(description) =>
          onDecide(
            { action: "describe", description },
            "Could not save that description.",
          ).then((ok) => ok && setEditing(false))
        }
        onCancel={() => setEditing(false)}
      />
      {(detail || (item.status === "proposed" && item.reason)) && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          {[detail, item.reason?.replaceAll(item.kind, words(item.kind))]
            .filter(Boolean)
            .join(" ")}
        </p>
      )}
      {isOwner && !editing && (
        <div className="flex flex-wrap items-center gap-1.5">
          {item.status !== "approved" && (
            <button
              type="button"
              className="btn btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide({ action: "approve" }, "Could not approve that kind.")
              }
            >
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {!item.built_in && item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide(
                  { action: "reject", merge_into: suggestedMerge },
                  "Could not reject that kind.",
                )
              }
            >
              <Duo icon={XIcon} tone="none" />
              {suggestedMerge
                ? `Reject and merge into ${words(suggestedMerge)}`
                : "Reject"}
            </button>
          )}
          {!item.built_in && targets.length > 0 && (
            <span className="flex items-center gap-1.5">
              <Duo icon={ArrowsMergeIcon} tone="none" />
              <Select
                className="h-8 text-[13px] w-[160px]"
                aria-label={`Merge ${words(item.kind)} into`}
                placeholder="Merge into…"
                value={
                  item.status === "rejected" ? (item.merge_into ?? "") : ""
                }
                disabled={busy}
                options={targets}
                onChange={(to) =>
                  onDecide(
                    { action: "reject", merge_into: to },
                    "Could not merge that kind.",
                  )
                }
              />
            </span>
          )}
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={busy}
            onClick={() => setEditing(true)}
          >
            <Duo icon={PencilSimpleIcon} tone="none" />
            {item.description ? "Edit description" : "Add description"}
          </button>
        </div>
      )}
      {children}
    </li>
  );
}

const DATATYPE: Record<PropertyItem["datatype"], string> = {
  text: "text",
  number: "number",
  integer: "whole number",
  money: "money",
  percent: "percent",
  bool: "yes or no",
  date: "date",
  datetime: "date and time",
  enum: "one of a list",
};

/** One typed property of a kind. Only an approved one takes values. */
export function PropertyRow({
  item,
  isOwner,
  busy,
  targets,
  onDecide,
}: {
  item: PropertyItem;
  isOwner: boolean;
  busy: boolean;
  targets: { value: string; label: string }[];
  onDecide: Decide<PropertyDecision>;
}) {
  const [editing, setEditing] = useState(false);
  const type = [DATATYPE[item.datatype], item.unit].filter(Boolean).join(" · ");
  const values = `${item.values} ${item.values === 1 ? "value" : "values"}`;
  const suggestedMerge =
    item.status === "proposed" && item.merge_into ? item.merge_into : null;
  const detail =
    item.status === "rejected"
      ? item.merge_into
        ? `New values go to ${words(item.merge_into)}.`
        : "New values are refused."
      : suggestedMerge
        ? `Suggested: merge into ${words(suggestedMerge)}.`
        : null;
  return (
    <li className="flex flex-col gap-1 py-1.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="flex-1 min-w-[160px] flex flex-wrap items-baseline gap-x-2">
          <span className="text-[13px] font-medium">{words(item.name)}</span>
          <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {type} · {values}
          </span>
        </span>
        <StatusLabel tone={STATUS[item.status].tone}>
          {STATUS[item.status].label}
        </StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={item.samples}
        editing={editing}
        busy={busy}
        onSave={(description) =>
          onDecide(
            { action: "describe", description },
            "Could not save that description.",
          ).then((ok) => ok && setEditing(false))
        }
        onCancel={() => setEditing(false)}
      />
      {detail && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          {detail}
          {item.reason ? ` ${item.reason}` : ""}
        </p>
      )}
      {isOwner && !editing && (
        <div className="flex flex-wrap items-center gap-1.5">
          {item.status !== "approved" && (
            <button
              type="button"
              className="btn btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide(
                  { action: "approve" },
                  "Could not approve that property.",
                )
              }
            >
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide(
                  { action: "reject", merge_into: suggestedMerge },
                  "Could not reject that property.",
                )
              }
            >
              <Duo icon={XIcon} tone="none" />
              {suggestedMerge
                ? `Reject and merge into ${words(suggestedMerge)}`
                : "Reject"}
            </button>
          )}
          {targets.length > 0 && (
            <span className="flex items-center gap-1.5">
              <Duo icon={ArrowsMergeIcon} tone="none" />
              <Select
                className="h-8 text-[13px] w-[160px]"
                aria-label={`Merge ${words(item.name)} into`}
                placeholder="Merge into…"
                value={
                  item.status === "rejected" ? (item.merge_into ?? "") : ""
                }
                disabled={busy}
                options={targets}
                onChange={(to) =>
                  onDecide(
                    { action: "reject", merge_into: to },
                    "Could not merge that property.",
                  )
                }
              />
            </span>
          )}
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={busy}
            onClick={() => setEditing(true)}
          >
            <Duo icon={PencilSimpleIcon} tone="none" />
            {item.description ? "Edit description" : "Add description"}
          </button>
        </div>
      )}
    </li>
  );
}

/** One metric of a kind: what it computes in plain words, and a run of it on demand. Only an approved
 * one runs by name for AI apps; "Try it" runs it here whatever its status. */
export function MetricRow({
  item,
  isOwner,
  busy,
  targets,
  onDecide,
  onTry,
}: {
  item: MetricItem;
  isOwner: boolean;
  busy: boolean;
  targets: { value: string; label: string }[];
  onDecide: Decide<MetricDecision>;
  onTry: () => Promise<MetricRun | { error: string }>;
}) {
  const [editing, setEditing] = useState(false);
  const [tried, setTried] = useState<MetricRun | { error: string } | null>(
    null,
  );
  const [trying, setTrying] = useState(false);
  const suggestedMerge =
    item.status === "proposed" && item.merge_into ? item.merge_into : null;
  const detail =
    item.status === "rejected"
      ? item.merge_into
        ? `Runs ${words(item.merge_into)} instead.`
        : "AI apps cannot run it."
      : suggestedMerge
        ? `Suggested: merge into ${words(suggestedMerge)}.`
        : null;
  async function doTry() {
    setTrying(true);
    try {
      setTried(await onTry());
    } catch {
      setTried({ error: "Could not run it. Try again." });
    } finally {
      setTrying(false);
    }
  }
  return (
    <li className="flex flex-col gap-1 py-1.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="flex-1 min-w-[160px] flex flex-wrap items-baseline gap-x-2">
          <span className="text-[13px] font-medium">{words(item.name)}</span>
          <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {item.expr_words}
            {item.unit ? ` · ${item.unit}` : ""}
          </span>
        </span>
        <StatusLabel tone={STATUS[item.status].tone}>
          {STATUS[item.status].label}
        </StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={[]}
        editing={editing}
        busy={busy}
        onSave={(description) =>
          onDecide(
            { action: "describe", description },
            "Could not save that description.",
          ).then((ok) => ok && setEditing(false))
        }
        onCancel={() => setEditing(false)}
      />
      {detail && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          {detail}
          {item.reason ? ` ${item.reason}` : ""}
        </p>
      )}
      {!editing && (
        <div className="flex flex-wrap items-center gap-1.5">
          {isOwner && item.status !== "approved" && (
            <button
              type="button"
              className="btn btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide(
                  { action: "approve" },
                  "Could not approve that metric.",
                )
              }
            >
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {isOwner && item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide(
                  { action: "reject", merge_into: suggestedMerge },
                  "Could not reject that metric.",
                )
              }
            >
              <Duo icon={XIcon} tone="none" />
              {suggestedMerge
                ? `Reject and merge into ${words(suggestedMerge)}`
                : "Reject"}
            </button>
          )}
          {isOwner && targets.length > 0 && (
            <span className="flex items-center gap-1.5">
              <Duo icon={ArrowsMergeIcon} tone="none" />
              <Select
                className="h-8 text-[13px] w-[160px]"
                aria-label={`Merge ${words(item.name)} into`}
                placeholder="Merge into…"
                value={
                  item.status === "rejected" ? (item.merge_into ?? "") : ""
                }
                disabled={busy}
                options={targets}
                onChange={(to) =>
                  onDecide(
                    { action: "reject", merge_into: to },
                    "Could not merge that metric.",
                  )
                }
              />
            </span>
          )}
          {isOwner && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() => setEditing(true)}
            >
              <Duo icon={PencilSimpleIcon} tone="none" />
              {item.description ? "Edit description" : "Add description"}
            </button>
          )}
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={trying}
            onClick={doTry}
          >
            <Duo icon={PlayIcon} tone="none" />
            {trying ? "Running…" : "Try it"}
          </button>
        </div>
      )}
      {tried && "error" in tried && (
        <p
          role="status"
          className="text-[12px]"
          style={{ color: "var(--ink-dim)" }}
        >
          {tried.error}
        </p>
      )}
      {tried && !("error" in tried) && (
        <div
          role="status"
          className="soft-panel px-3 py-2.5 flex flex-col gap-1.5 max-w-md"
        >
          <MetricNumber run={tried} size={22} />
          <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            As of {asOfDate(tried.as_of)}, from{" "}
            <span className="font-mono">{tried.n_entities}</span>{" "}
            {tried.n_entities === 1 ? "entry" : "entries"}
            {tried.status !== "approved"
              ? ". Not approved yet, so AI apps cannot use this number."
              : "."}
          </p>
          <MetricWarnings
            warnings={tried.warnings.filter(
              (w) => !w.includes("is proposed, not approved"),
            )}
          />
        </div>
      )}
    </li>
  );
}

export function LinkRow({
  item,
  isOwner,
  busy,
  onDecide,
}: {
  item: LinkItem;
  isOwner: boolean;
  busy: boolean;
  onDecide: Decide<
    | { action: "approve" }
    | { action: "reject" }
    | { action: "describe"; description: string }
  >;
}) {
  const [editing, setEditing] = useState(false);
  const samples = item.samples.map(
    (s) => `${s.from} ${words(item.label)} ${s.to}`,
  );
  return (
    <li className="flex flex-col gap-1.5 py-2.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <span className="flex-1 min-w-[160px] flex flex-wrap items-baseline gap-x-2">
          <span className="text-[14px]">
            {words(item.from_kind)}{" "}
            <span className="font-medium">{words(item.label)}</span>{" "}
            {words(item.to_kind)}
          </span>
          <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {item.count} {item.count === 1 ? "link" : "links"}
          </span>
        </span>
        <StatusLabel tone={STATUS[item.status].tone}>
          {STATUS[item.status].label}
        </StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={samples}
        editing={editing}
        busy={busy}
        onSave={(description) =>
          onDecide(
            { action: "describe", description },
            "Could not save that description.",
          ).then((ok) => ok && setEditing(false))
        }
        onCancel={() => setEditing(false)}
      />
      {item.status === "proposed" && item.reason && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          <MatchShare reason={item.reason} />
        </p>
      )}
      {isOwner && !editing && (
        <div className="flex flex-wrap items-center gap-1.5">
          {item.status !== "approved" && (
            <button
              type="button"
              className="btn btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide({ action: "approve" }, "Could not approve that link.")
              }
            >
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() =>
                onDecide({ action: "reject" }, "Could not reject that link.")
              }
            >
              <Duo icon={XIcon} tone="none" />
              Reject
            </button>
          )}
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={busy}
            onClick={() => setEditing(true)}
          >
            <Duo icon={PencilSimpleIcon} tone="none" />
            {item.description ? "Edit description" : "Add description"}
          </button>
        </div>
      )}
    </li>
  );
}

/** The description (or a few examples when there is none), or the editor for it. */
function Description({
  text,
  samples,
  editing,
  busy,
  onSave,
  onCancel,
}: {
  text: string | null | undefined;
  samples: string[];
  editing: boolean;
  busy: boolean;
  onSave: (description: string) => void;
  onCancel: () => void;
}) {
  if (editing)
    return (
      <DescriptionEditor
        text={text ?? ""}
        busy={busy}
        onSave={onSave}
        onCancel={onCancel}
      />
    );
  if (text) return <p className="text-[13px]">{text}</p>;
  if (samples.length === 0) return null;
  return (
    <p className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
      For example: {samples.join(", ")}
    </p>
  );
}

// Mounted when editing starts, so it opens on the current description.
function DescriptionEditor({
  text,
  busy,
  onSave,
  onCancel,
}: {
  text: string;
  busy: boolean;
  onSave: (description: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(text);
  return (
    <form
      className="flex flex-wrap items-center gap-1.5"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(value.trim());
      }}
    >
      <input
        aria-label="Description"
        autoFocus
        maxLength={500}
        className="field h-9 px-3 text-[13px] flex-1 min-w-[200px]"
        placeholder="What this is, in one sentence"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      />
      <button type="submit" className="btn btn-sm" disabled={busy}>
        {busy ? "Saving…" : "Save"}
      </button>
      <button type="button" className="btn btn-ghost btn-sm" onClick={onCancel}>
        Cancel
      </button>
    </form>
  );
}
