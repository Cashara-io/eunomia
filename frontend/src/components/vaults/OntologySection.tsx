"use client";

// A space's kinds and links: what its memories are about (person, customer, decision, ...) and how
// they connect (works at, owns, ...). AI apps and the server's model only suggest; an owner decides.
import { useState, type ReactNode } from "react";
import { ArrowsMergeIcon, CheckIcon, PencilSimpleIcon, PlayIcon, SparkleIcon, XIcon } from "@phosphor-icons/react/ssr";
import { asOfDate, MetricNumber, MetricWarnings } from "@/components/chat/views/MetricView";
import type { MetricRun } from "@/components/chat/views/types";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import Duo from "@/components/ui/Duo";
import StatusLabel, { type StatusTone } from "@/components/ui/StatusLabel";
import {
  useDecideKind,
  useDecideLink,
  useDecideMetric,
  useDecideProperty,
  useOntology,
  useReadiness,
  useSetStrict,
  useSuggestOntology,
  useTryMetric,
  type KindItem,
  type LinkItem,
  type MetricItem,
  type PropertyItem,
  type ViewStatus,
} from "@/lib/queries/ontology";
import type { MetricDecision, PropertyDecision } from "@/lib/gen";
import type { Vault } from "@/lib/types";
import AutomapBar, { MatchShare } from "./AutomapBar";
import { Disclosure } from "./bits";

const STATUS: Record<ViewStatus, { tone: StatusTone; label: string }> = {
  observed: { tone: "neutral", label: "In use" },
  proposed: { tone: "info", label: "Suggested" },
  approved: { tone: "good", label: "Approved" },
  rejected: { tone: "warning", label: "Rejected" },
};

const words = (name: string) => name.replace(/_/g, " ");

/** The ontology is read only once the section is opened: it reads every relation in the space. */
export default function OntologySection({ vault }: { vault: Vault }) {
  const cached = useOntology(vault.id, false).data;
  const proposed = cached ? waiting(cached) : 0;
  return (
    <Disclosure
      title="Kinds and links"
      hint={proposed > 0 ? `${proposed} ${proposed === 1 ? "suggestion" : "suggestions"} waiting for review` : "What the memories in this space are about, and how they connect"}
    >
      <OntologyBody vault={vault} />
    </Disclosure>
  );
}

const waiting = (data: { kinds: KindItem[]; links: LinkItem[] }) =>
  [...data.kinds, ...data.links, ...data.kinds.flatMap((k) => k.properties), ...data.kinds.flatMap((k) => k.metrics)].filter((x) => x.status === "proposed").length;

function OntologyBody({ vault }: { vault: Vault }) {
  const isOwner = vault.role === "owner";
  const query = useOntology(vault.id);
  const suggest = useSuggestOntology(vault.id);
  const decideKind = useDecideKind(vault.id);
  const decideLink = useDecideLink(vault.id);
  const decideProperty = useDecideProperty(vault.id);
  const decideMetric = useDecideMetric(vault.id);
  const tryMetric = useTryMetric(vault.id);
  const [error, setError] = useState<Failure | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const data = query.data;

  /** True when it worked; a failure shows under the heading. */
  async function run(action: () => Promise<unknown>, fallback: string): Promise<boolean> {
    setError(null);
    try {
      await action();
      return true;
    } catch (e) {
      setError(failure(e, fallback));
      return false;
    }
  }

  async function doSuggest() {
    setNote(null);
    await run(async () => {
      const out = await suggest.mutateAsync();
      if (out.mode === "model") {
        const n = [...out.proposed.kinds, ...out.proposed.links, ...out.proposed.properties, ...out.proposed.metrics].filter((x) => x.status === "proposed").length;
        setNote(n === 0 ? "Nothing new to suggest." : `${n} ${n === 1 ? "suggestion" : "suggestions"} to review below.`);
      } else {
        setNote(
          out.note
            ? "The model could not be reached, so nothing was suggested. Try again later, or ask your AI app to review the kinds."
            : "No model is set up on this server, so nothing was suggested. Ask your AI app to review the kinds: it can suggest them for you to approve here.",
        );
      }
    }, "Could not get suggestions. Try again.");
  }

  // Where a rejected kind's memories can go: any kind that is kept, except itself.
  const targets = (self: string) =>
    (data?.kinds ?? [])
      .filter((k) => k.kind !== self && k.status !== "rejected" && (k.built_in || k.status === "approved" || k.entities > 0))
      .map((k) => ({ value: k.kind, label: words(k.kind) }));

  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <p className="text-[13px] flex-1 min-w-[220px]" style={{ color: "var(--ink-dim)" }}>
          {isOwner
            ? "AI apps suggest kinds; you decide. A rejected kind's new memories go to the kind you merge it into."
            : "AI apps suggest kinds and the space's owners decide. You can see what was decided."}
        </p>
        <button type="button" className="btn btn-sm shrink-0" onClick={doSuggest} disabled={suggest.isPending}>
          <Duo icon={SparkleIcon} tone="none" />
          {suggest.isPending ? "Suggesting…" : "Suggest kinds"}
        </button>
      </div>
      {note && (
        <p role="status" className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {note}
        </p>
      )}
      <ReadinessPanel vaultId={vault.id} />
      {data && <StrictSwitch vaultId={vault.id} on={data.strict} isOwner={isOwner} onError={setError} />}
      {isOwner && <AutomapBar vaultId={vault.id} />}
      {error && <ErrorLine error={error} />}
      {query.isError && <ErrorLine error={failure(query.error, "Could not load the kinds in this space.")} />}

      <h4 className="text-[13px] font-medium" style={{ color: "var(--ink-dim)" }}>
        Kinds
      </h4>
      <ul className="hairline-rows" aria-label="Kinds in this space">
        {!data && !query.isError && [0, 1, 2].map((i) => <SkeletonRow key={i} />)}
        {data?.kinds.filter((k) => !(k.built_in && k.entities === 0 && k.status === "observed" && k.properties.length === 0 && k.metrics.length === 0)).map((k) => (
          <KindRow
            key={k.kind}
            item={k}
            isOwner={isOwner}
            busy={decideKind.isPending}
            targets={targets(k.kind)}
            onDecide={(decision, fallback) => run(() => decideKind.mutateAsync({ kind: k.kind, decision }), fallback)}
          >
            {k.properties.length > 0 && (
              <ul className="flex flex-col gap-1 pl-3 border-l" style={{ borderColor: "var(--border)" }} aria-label={`Properties of ${words(k.kind)}`}>
                {k.properties.map((p) => (
                  <PropertyRow
                    key={p.name}
                    item={p}
                    isOwner={isOwner}
                    busy={decideProperty.isPending}
                    targets={k.properties.filter((o) => o.name !== p.name && o.status !== "rejected").map((o) => ({ value: o.name, label: words(o.name) }))}
                    onDecide={(decision, fallback) => run(() => decideProperty.mutateAsync({ kind: k.kind, name: p.name, decision }), fallback)}
                  />
                ))}
              </ul>
            )}
            {k.metrics.length > 0 && (
              <ul className="flex flex-col gap-1 pl-3 border-l" style={{ borderColor: "var(--border)" }} aria-label={`Metrics of ${words(k.kind)}`}>
                {k.metrics.map((m) => (
                  <MetricRow
                    key={m.name}
                    item={m}
                    isOwner={isOwner}
                    busy={decideMetric.isPending}
                    targets={k.metrics.filter((o) => o.name !== m.name && o.status !== "rejected").map((o) => ({ value: o.name, label: words(o.name) }))}
                    onDecide={(decision, fallback) => run(() => decideMetric.mutateAsync({ kind: k.kind, name: m.name, decision }), fallback)}
                    onTry={() => tryMetric.mutateAsync({ kind: k.kind, name: m.name })}
                  />
                ))}
              </ul>
            )}
          </KindRow>
        ))}
      </ul>

      <h4 className="text-[13px] font-medium" style={{ color: "var(--ink-dim)" }}>
        Links
      </h4>
      <ul className="hairline-rows" aria-label="Links in this space">
        {!data && !query.isError && [0, 1].map((i) => <SkeletonRow key={i} />)}
        {data?.links.length === 0 && (
          <li className="text-[13px] py-2" style={{ color: "var(--ink-dim)" }}>
            No links yet. They appear when memories connect two things, like a person who works at a company.
          </li>
        )}
        {data?.links.map((l) => (
          <LinkRow
            key={`${l.from_kind} ${l.label} ${l.to_kind}`}
            item={l}
            isOwner={isOwner}
            busy={decideLink.isPending}
            onDecide={(decision, fallback) =>
              run(() => decideLink.mutateAsync({ from_kind: l.from_kind, label: l.label, to_kind: l.to_kind, decision }), fallback)
            }
          />
        ))}
      </ul>
    </>
  );
}

const counted = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** Whether the space can answer number questions yet, and what still stands in the way. */
function ReadinessPanel({ vaultId }: { vaultId: string }) {
  const query = useReadiness(vaultId);
  const r = query.data;
  if (query.isError) return <ErrorLine error={failure(query.error, "Could not check whether this space is ready.")} />;
  if (!r) return <span className="skeleton h-12 w-full" />;
  const approved = [
    counted(r.kinds.approved, "kind", "kinds"),
    counted(r.properties.approved, "property", "properties"),
    counted(r.metrics.approved, "metric", "metrics"),
    counted(r.links.approved, "link", "links"),
  ].join(", ");
  return (
    <section aria-label="Ready for numbers" className="flex flex-col gap-1.5 py-1">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <StatusLabel tone={r.ready ? "good" : "warning"}>{r.ready ? "Ready to answer numbers" : "Not ready to answer every number yet"}</StatusLabel>
        <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          Approved: {approved}
        </span>
      </div>
      {r.blockers.length > 0 && (
        <ul className="flex flex-col gap-0.5 text-[13px] list-disc pl-5" aria-label="What is in the way">
          {r.blockers.slice(0, 5).map((b) => (
            <li key={b}>{b}</li>
          ))}
          {r.blockers.length > 5 && <li style={{ color: "var(--ink-dim)" }}>and {r.blockers.length - 5} more</li>}
        </ul>
      )}
    </section>
  );
}

/** "Only approved kinds and links": an owner's switch; members see whether it is on. */
function StrictSwitch({ vaultId, on, isOwner, onError }: { vaultId: string; on: boolean; isOwner: boolean; onError: (f: Failure | null) => void }) {
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
    <label htmlFor={`strict-${vaultId}`} className={`flex items-start gap-3 max-w-[68ch] ${isOwner ? "cursor-pointer" : ""}`}>
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
        <span className="text-[14px] font-medium">Only approved kinds and links</span>
        <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {on
            ? "On. AI apps can only save things under the kinds and links approved here. Anything else is turned away and shows up below as a suggestion to review."
            : "Off. AI apps can save things under any kind or link; new ones show up below as in use. Turn this on to accept only what you approved."}
          {!isOwner && " Only an owner of this space can change it."}
        </span>
      </span>
    </label>
  );
}

function SkeletonRow() {
  return (
    <li className="flex items-center gap-2 h-11">
      <span className="skeleton h-4 w-24" />
      <span className="skeleton h-4 flex-1" />
    </li>
  );
}

type Decide<D> = (decision: D, fallback: string) => Promise<boolean>;

function KindRow({
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
  onDecide: Decide<{ action: "approve" } | { action: "reject"; merge_into?: string | null } | { action: "describe"; description: string }>;
  children?: ReactNode;
}) {
  const [editing, setEditing] = useState(false);
  const count = `${item.entities} ${item.entities === 1 ? "entry" : "entries"}`;
  const suggestedMerge = item.status === "proposed" && item.merge_into ? item.merge_into : null;
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
        <StatusLabel tone={STATUS[item.status].tone}>{STATUS[item.status].label}</StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={item.samples}
        editing={editing}
        busy={busy}
        onSave={(description) => onDecide({ action: "describe", description }, "Could not save that description.").then((ok) => ok && setEditing(false))}
        onCancel={() => setEditing(false)}
      />
      {(detail || (item.status === "proposed" && item.reason)) && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          {[detail, item.reason].filter(Boolean).join(" ")}
        </p>
      )}
      {isOwner && !editing && (
        <div className="flex flex-wrap items-center gap-1.5">
          {item.status !== "approved" && (
            <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onDecide({ action: "approve" }, "Could not approve that kind.")}>
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {!item.built_in && item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() => onDecide({ action: "reject", merge_into: suggestedMerge }, "Could not reject that kind.")}
            >
              <Duo icon={XIcon} tone="none" />
              {suggestedMerge ? `Reject and merge into ${words(suggestedMerge)}` : "Reject"}
            </button>
          )}
          {!item.built_in && targets.length > 0 && (
            <span className="flex items-center gap-1.5">
              <Duo icon={ArrowsMergeIcon} tone="none" />
              <Select
                className="h-8 text-[13px] w-[160px]"
                aria-label={`Merge ${words(item.kind)} into`}
                placeholder="Merge into…"
                value={item.status === "rejected" ? (item.merge_into ?? "") : ""}
                disabled={busy}
                options={targets}
                onChange={(to) => onDecide({ action: "reject", merge_into: to }, "Could not merge that kind.")}
              />
            </span>
          )}
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setEditing(true)}>
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
function PropertyRow({
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
  const suggestedMerge = item.status === "proposed" && item.merge_into ? item.merge_into : null;
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
        <StatusLabel tone={STATUS[item.status].tone}>{STATUS[item.status].label}</StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={item.samples}
        editing={editing}
        busy={busy}
        onSave={(description) => onDecide({ action: "describe", description }, "Could not save that description.").then((ok) => ok && setEditing(false))}
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
            <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onDecide({ action: "approve" }, "Could not approve that property.")}>
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() => onDecide({ action: "reject", merge_into: suggestedMerge }, "Could not reject that property.")}
            >
              <Duo icon={XIcon} tone="none" />
              {suggestedMerge ? `Reject and merge into ${words(suggestedMerge)}` : "Reject"}
            </button>
          )}
          {targets.length > 0 && (
            <span className="flex items-center gap-1.5">
              <Duo icon={ArrowsMergeIcon} tone="none" />
              <Select
                className="h-8 text-[13px] w-[160px]"
                aria-label={`Merge ${words(item.name)} into`}
                placeholder="Merge into…"
                value={item.status === "rejected" ? (item.merge_into ?? "") : ""}
                disabled={busy}
                options={targets}
                onChange={(to) => onDecide({ action: "reject", merge_into: to }, "Could not merge that property.")}
              />
            </span>
          )}
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setEditing(true)}>
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
function MetricRow({
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
  const [tried, setTried] = useState<MetricRun | { error: string } | null>(null);
  const [trying, setTrying] = useState(false);
  const suggestedMerge = item.status === "proposed" && item.merge_into ? item.merge_into : null;
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
        <StatusLabel tone={STATUS[item.status].tone}>{STATUS[item.status].label}</StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={[]}
        editing={editing}
        busy={busy}
        onSave={(description) => onDecide({ action: "describe", description }, "Could not save that description.").then((ok) => ok && setEditing(false))}
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
            <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onDecide({ action: "approve" }, "Could not approve that metric.")}>
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {isOwner && item.status !== "rejected" && (
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              disabled={busy}
              onClick={() => onDecide({ action: "reject", merge_into: suggestedMerge }, "Could not reject that metric.")}
            >
              <Duo icon={XIcon} tone="none" />
              {suggestedMerge ? `Reject and merge into ${words(suggestedMerge)}` : "Reject"}
            </button>
          )}
          {isOwner && targets.length > 0 && (
            <span className="flex items-center gap-1.5">
              <Duo icon={ArrowsMergeIcon} tone="none" />
              <Select
                className="h-8 text-[13px] w-[160px]"
                aria-label={`Merge ${words(item.name)} into`}
                placeholder="Merge into…"
                value={item.status === "rejected" ? (item.merge_into ?? "") : ""}
                disabled={busy}
                options={targets}
                onChange={(to) => onDecide({ action: "reject", merge_into: to }, "Could not merge that metric.")}
              />
            </span>
          )}
          {isOwner && (
            <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setEditing(true)}>
              <Duo icon={PencilSimpleIcon} tone="none" />
              {item.description ? "Edit description" : "Add description"}
            </button>
          )}
          <button type="button" className="btn btn-ghost btn-sm" disabled={trying} onClick={doTry}>
            <Duo icon={PlayIcon} tone="none" />
            {trying ? "Running…" : "Try it"}
          </button>
        </div>
      )}
      {tried && "error" in tried && (
        <p role="status" className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          {tried.error}
        </p>
      )}
      {tried && !("error" in tried) && (
        <div role="status" className="soft-panel px-3 py-2.5 flex flex-col gap-1.5 max-w-md">
          <MetricNumber run={tried} size={22} />
          <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            As of {asOfDate(tried.as_of)}, from <span className="font-mono">{tried.n_entities}</span> {tried.n_entities === 1 ? "entry" : "entries"}
            {tried.status !== "approved" ? ". Not approved yet, so AI apps cannot use this number." : "."}
          </p>
          <MetricWarnings warnings={tried.warnings.filter((w) => !w.includes("is proposed, not approved"))} />
        </div>
      )}
    </li>
  );
}

function LinkRow({
  item,
  isOwner,
  busy,
  onDecide,
}: {
  item: LinkItem;
  isOwner: boolean;
  busy: boolean;
  onDecide: Decide<{ action: "approve" } | { action: "reject" } | { action: "describe"; description: string }>;
}) {
  const [editing, setEditing] = useState(false);
  const samples = item.samples.map((s) => `${s.from} ${words(item.label)} ${s.to}`);
  return (
    <li className="flex flex-col gap-1.5 py-2.5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <span className="flex-1 min-w-[160px] flex flex-wrap items-baseline gap-x-2">
          <span className="text-[14px]">
            {words(item.from_kind)} <span className="font-medium">{words(item.label)}</span> {words(item.to_kind)}
          </span>
          <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {item.count} {item.count === 1 ? "link" : "links"}
          </span>
        </span>
        <StatusLabel tone={STATUS[item.status].tone}>{STATUS[item.status].label}</StatusLabel>
      </div>
      <Description
        text={item.description}
        samples={samples}
        editing={editing}
        busy={busy}
        onSave={(description) => onDecide({ action: "describe", description }, "Could not save that description.").then((ok) => ok && setEditing(false))}
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
            <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onDecide({ action: "approve" }, "Could not approve that link.")}>
              <Duo icon={CheckIcon} tone="none" />
              Approve
            </button>
          )}
          {item.status !== "rejected" && (
            <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => onDecide({ action: "reject" }, "Could not reject that link.")}>
              <Duo icon={XIcon} tone="none" />
              Reject
            </button>
          )}
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setEditing(true)}>
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
  if (editing) return <DescriptionEditor text={text ?? ""} busy={busy} onSave={onSave} onCancel={onCancel} />;
  if (text) return <p className="text-[13px]">{text}</p>;
  if (samples.length === 0) return null;
  return (
    <p className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
      For example: {samples.join(", ")}
    </p>
  );
}

// Mounted when editing starts, so it opens on the current description.
function DescriptionEditor({ text, busy, onSave, onCancel }: { text: string; busy: boolean; onSave: (description: string) => void; onCancel: () => void }) {
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
