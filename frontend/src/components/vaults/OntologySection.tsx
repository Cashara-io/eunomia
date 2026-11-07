"use client";

// A space's kinds and links: what its memories are about (person, customer, decision, ...) and how
// they connect (works at, owns, ...). AI apps and the server's model only suggest; an owner decides.
import { useState } from "react";
import { ArrowsMergeIcon, CheckIcon, PencilSimpleIcon, SparkleIcon, XIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import Duo from "@/components/ui/Duo";
import StatusLabel, { type StatusTone } from "@/components/ui/StatusLabel";
import {
  useDecideKind,
  useDecideLink,
  useOntology,
  useSuggestOntology,
  type KindItem,
  type LinkItem,
  type ViewStatus,
} from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import { Disclosure } from "./bits";

const STATUS: Record<ViewStatus, { tone: StatusTone; label: string }> = {
  observed: { tone: "neutral", label: "In use" },
  proposed: { tone: "info", label: "Suggested" },
  approved: { tone: "good", label: "Approved" },
  rejected: { tone: "warning", label: "Rejected" },
};

const words = (name: string) => name.replace(/_/g, " ");

export default function OntologySection({ vault }: { vault: Vault }) {
  const isOwner = vault.role === "owner";
  const query = useOntology(vault.id);
  const suggest = useSuggestOntology(vault.id);
  const decideKind = useDecideKind(vault.id);
  const decideLink = useDecideLink(vault.id);
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
        const n = out.proposed.kinds.filter((k) => k.status === "proposed").length + out.proposed.links.filter((l) => l.status === "proposed").length;
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

  const proposed = (data?.kinds ?? []).filter((k) => k.status === "proposed").length + (data?.links ?? []).filter((l) => l.status === "proposed").length;

  return (
    <Disclosure
      title="Kinds and links"
      hint={proposed > 0 ? `${proposed} ${proposed === 1 ? "suggestion" : "suggestions"} waiting for review` : "What the memories in this space are about, and how they connect"}
    >
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
      {error && <ErrorLine error={error} />}
      {query.isError && <ErrorLine error={failure(query.error, "Could not load the kinds in this space.")} />}

      <h4 className="text-[13px] font-medium" style={{ color: "var(--ink-dim)" }}>
        Kinds
      </h4>
      <ul className="hairline-rows" aria-label="Kinds in this space">
        {!data && !query.isError && [0, 1, 2].map((i) => <SkeletonRow key={i} />)}
        {data?.kinds.filter((k) => !(k.built_in && k.entities === 0 && k.status === "observed")).map((k) => (
          <KindRow
            key={k.kind}
            item={k}
            isOwner={isOwner}
            busy={decideKind.isPending}
            targets={targets(k.kind)}
            onDecide={(decision, fallback) => run(() => decideKind.mutateAsync({ kind: k.kind, decision }), fallback)}
          />
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
    </Disclosure>
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
}: {
  item: KindItem;
  isOwner: boolean;
  busy: boolean;
  targets: { value: string; label: string }[];
  onDecide: Decide<{ action: "approve" } | { action: "reject"; merge_into?: string | null } | { action: "describe"; description: string }>;
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
      {detail && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          {detail}
          {item.reason ? ` ${item.reason}` : ""}
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
