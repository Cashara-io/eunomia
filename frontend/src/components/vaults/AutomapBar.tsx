"use client";

// Auto-map: Eunomia profiles the fields of the space's connected records, or one uploaded CSV, and the
// server's model proposes kinds, properties and links. Every link was checked against the values first,
// so each one says how much of it matched. An owner still decides each proposal in the list below.
import { useState } from "react";
import { TreeStructureIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import Duo from "@/components/ui/Duo";
import StatusLabel from "@/components/ui/StatusLabel";
import { useVaultDocuments } from "@/lib/queries/documents";
import { useAutomap, type AutomapOut, type LinkCheck } from "@/lib/queries/ontology";

const RECORDS = "records";
const words = (name: string) => name.replace(/_/g, " ");

/** "82% of values match (41 of 50)", with the numbers in mono; any other reason as it is. */
export function MatchShare({ reason }: { reason: string }) {
  const m = /^(\d+)% of values match \((\d+) of (\d+)\)$/.exec(reason);
  if (!m) return <>{reason}</>;
  return (
    <>
      <span className="font-mono">{m[1]}%</span> of values match (<span className="font-mono">{m[2]}</span> of{" "}
      <span className="font-mono">{m[3]}</span>)
    </>
  );
}

function LinkChecks({ checks }: { checks: LinkCheck[] }) {
  if (checks.length === 0) return null;
  return (
    <ul className="hairline-rows max-h-72 overflow-y-auto" aria-label="Links checked against the values">
      {checks.map((c) => (
        <li key={`${c.record_type} ${c.field} ${c.label}`} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
          <span className="flex-1 min-w-[200px] flex flex-col">
            <span className="text-[13px]">
              {words(c.from_kind)} <span className="font-medium">{words(c.label)}</span> {words(c.to_kind)}
            </span>
            <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
              From the {c.field} field: <MatchShare reason={c.reason} />
            </span>
          </span>
          <StatusLabel tone={c.kept ? "info" : "neutral"}>{c.kept ? "Suggested" : "Left out"}</StatusLabel>
        </li>
      ))}
    </ul>
  );
}

function summary(out: AutomapOut): string {
  if (out.mode === "profile_only") {
    return out.note?.startsWith("no records")
      ? "Nothing to map yet: connect a source or upload a CSV first."
      : out.note
        ? "The model could not be reached, so nothing was suggested. Try again later, or ask your AI app to map it."
        : "No model is set up on this server, so nothing was suggested. Ask your AI app to map it: it gets the same field profile and suggests kinds for you to approve here.";
  }
  const p = out.proposed;
  const n = [...p.kinds, ...p.links, ...p.properties].filter((x) => x.status === "proposed").length;
  const dropped = out.links.filter((l) => !l.kept).length;
  const left = dropped > 0 ? ` ${dropped} ${dropped === 1 ? "link was" : "links were"} left out because no value matched.` : "";
  return n === 0 ? `Nothing new to suggest.${left}` : `${n} ${n === 1 ? "suggestion" : "suggestions"} to review below.${left}`;
}

export default function AutomapBar({ vaultId }: { vaultId: string }) {
  const docs = useVaultDocuments(vaultId);
  const automap = useAutomap(vaultId);
  const [target, setTarget] = useState(RECORDS);
  const [out, setOut] = useState<AutomapOut | null>(null);
  const [error, setError] = useState<Failure | null>(null);
  const csvs = (docs.data?.results ?? []).filter((d) => d.media_type === "text/csv");
  const options = [{ value: RECORDS, label: "Connected records" }, ...csvs.map((d) => ({ value: d.id, label: d.filename }))];

  async function run() {
    setError(null);
    setOut(null);
    try {
      setOut(await automap.mutateAsync(target === RECORDS ? {} : { document_id: target }));
    } catch (e) {
      setError(failure(e, "Could not auto-map. Try again."));
    }
  }

  return (
    <div className="flex flex-col gap-2" data-testid="automap">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <p className="text-[13px] flex-1 min-w-[220px]" style={{ color: "var(--ink-dim)" }}>
          Auto-map reads the fields of your records or a CSV and suggests kinds, properties and links. Links are checked against the values first.
        </p>
        <Select className="h-8 text-[13px] w-[200px]" aria-label="What to auto-map" value={target} options={options} onChange={setTarget} disabled={automap.isPending} />
        <button type="button" className="btn btn-sm shrink-0" onClick={run} disabled={automap.isPending}>
          <Duo icon={TreeStructureIcon} tone="none" />
          {automap.isPending ? "Mapping…" : "Auto-map"}
        </button>
      </div>
      {out && (
        <p role="status" className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {summary(out)}
        </p>
      )}
      {out?.mode === "model" && <LinkChecks checks={out.links} />}
      {error && <ErrorLine error={error} />}
    </div>
  );
}
