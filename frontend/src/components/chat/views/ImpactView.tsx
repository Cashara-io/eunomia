"use client";
import { useState } from "react";
import { invoke } from "@/lib/queries/tools";
import { LightningIcon } from "@phosphor-icons/react/ssr";
import Duo from "@/components/ui/Duo";
import { EntityName, PathSentence, words } from "./bits";
import type { Affected, ImpactView as Impact } from "./types";

const away = (n: number) => (n === 1 ? "Directly linked" : `${n} links away`);

/** What a change to one entity affects: the start, then the affected entities grouped by how many
 * relations away they are, each with the chain that reaches it and the model's reason. */
export default function ImpactView({ view }: { view: Impact }) {
  const [loaded, setLoaded] = useState<Pick<Impact, "affected" | "pagination"> | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const current = loaded ?? view;
  const pagination = current.pagination;
  async function load(page: number) {
    if (!pagination || pending) return;
    setPending(true);
    setError("");
    try {
      const next = await invoke<Impact | { error: string }>("show_view", {
        view: "impact", start_id: view.start.id, headline: "Linked items",
        reach: pagination.reach, page,
      });
      if ("error" in next) throw new Error(next.error);
      setLoaded({ affected: next.affected, pagination: next.pagination });
    } catch {
      setError("Could not load linked items. Try again.");
    } finally { setPending(false); }
  }
  const groups = new Map<number, Affected[]>();
  for (const a of current.affected) groups.set(a.path.length, [...(groups.get(a.path.length) ?? []), a]);
  const steps = [...groups.keys()].sort((a, b) => a - b);
  return (
    <section className="ledger p-4 flex flex-col gap-3" aria-label="What this affects">
      <header className="flex items-start gap-2.5">
        <Duo icon={LightningIcon} size={18} tone="company" className="mt-0.5" />
        <div className="min-w-0 flex flex-col gap-1">
          <h3 className="section-title">{view.headline}</h3>
          <div className="flex flex-wrap items-center gap-x-1.5 text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Starting from <span className="pill !min-h-7 !px-2.5" style={{ color: "var(--ink)" }}><EntityName entity={view.start} strong /></span>
            <span>{words(view.start.kind)}</span>
          </div>
        </div>
      </header>
      {view.linked === false ? (
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }} data-testid="impact-no-links">
          {view.start.name} has no saved links yet, so there is nothing to follow. Link it to the things it touches, then ask again.
        </p>
      ) : (
        current.affected.length === 0 && !pagination && (
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Nothing else is affected.
          </p>
        )
      )}
      {pagination && <div className="flex flex-col gap-2">
        <p className="text-[13px]" role="status">
          {pagination.total.toLocaleString("en-AU")} linked items in this selection.
          {" "}Showing {current.affected.length === 0 ? 0 : pagination.page * pagination.page_size + 1} to {pagination.page * pagination.page_size + current.affected.length}.
          {pagination.truncated && " This page is only part of the full list."}
        </p>
        <div className="flex gap-2" aria-label="Impact pages">
          <button className="btn btn-sm" disabled={pending || pagination.page === 0} onClick={() => void load(pagination.page - 1)}>Previous</button>
          <button className="btn btn-sm" disabled={pending || !pagination.has_more} onClick={() => void load(pagination.page + 1)}>Next</button>
        </div>
        {pending && <p role="status">Loading linked items…</p>}
        {error && <p role="alert">{error}</p>}
      </div>}
      {steps.map((n) => (
        <div key={n} className="flex flex-col gap-2">
          <h4 className="label">{away(n)}</h4>
          <ul className="grid gap-2 sm:grid-cols-2">
            {groups.get(n)!.map((a) => (
              <li key={a.entity.id} className="soft-panel px-3 py-2.5 flex flex-col gap-1 min-w-0 text-[13px]">
                <div className="flex items-baseline justify-between gap-2 min-w-0 text-[14px]">
                  <EntityName entity={a.entity} strong />
                  <span className="label shrink-0">{words(a.entity.kind)}</span>
                </div>
                <PathSentence path={a.path} />
                {a.why && <p style={{ color: "var(--ink-dim)" }}>{a.why}</p>}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
