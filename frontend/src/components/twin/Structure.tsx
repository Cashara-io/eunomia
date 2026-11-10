"use client";

import Link from "next/link";
import { useState } from "react";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import StatusLabel from "@/components/ui/StatusLabel";
import type { Vault } from "@/lib/types";
import { useSuggestOntology, type OntologyOut } from "@/lib/queries/ontology";
import { STATUS, StrictSwitch, words } from "./DefinitionRows";
import {
  Filter,
  Pagination,
  Search,
  statusOptions,
  useTwinParams,
} from "./controls";
import {
  kindHref,
  pageNumber,
  PAGE_SIZE,
  twinHref,
  visibleKind,
} from "./model";

export default function Structure({
  vault,
  data,
}: {
  vault: Vault;
  data: OntologyOut;
}) {
  const { params } = useTwinParams();
  const suggest = useSuggestOntology(vault.id);
  const [error, setError] = useState<Failure | null>(null);
  const [note, setNote] = useState("");
  const q = (params.get("q") ?? "").toLowerCase();
  const status = params.get("status") ?? "all";
  const type = params.get("type") ?? "all";
  const rows = [
    ...data.kinds
      .filter(visibleKind)
      .map((k) => ({
        id: `kind-${k.kind}`,
        type: "kind",
        title: words(k.kind),
        kind: k.kind,
        detail: k.description ?? "No definition yet",
        count: k.entities,
        unit: "objects",
        status: k.status,
      })),
    ...data.links.map((l) => ({
      id: JSON.stringify(["link", l.from_kind, l.label, l.to_kind]),
      type: "link",
      title: `${words(l.from_kind)} → ${words(l.label)} → ${words(l.to_kind)}`,
      kind: l.from_kind,
      detail: l.description ?? "Directed link type",
      count: l.count,
      unit: "links",
      status: l.status,
    })),
  ].filter(
    (r) =>
      (type === "all" || r.type === type) &&
      (status === "all" || status === r.status) &&
      `${r.title} ${r.detail}`.toLowerCase().includes(q),
  );
  const page = pageNumber(params.get("page"), rows.length);
  async function doSuggest() {
    setError(null);
    setNote("");
    try {
      const out = await suggest.mutateAsync();
      setNote(
        out.mode === "model"
          ? "Suggestions refreshed. Open Review to decide what to keep."
          : out.note
            ? "The model could not be reached, so nothing was suggested. Try again later or ask your AI app to suggest kinds."
            : "No model is set up on this server. Ask your AI app to suggest kinds for you to review.",
      );
    } catch (e) {
      setError(failure(e, "Could not get suggestions. Try again."));
    }
  }
  return (
    <>
      <div className="flex flex-wrap justify-between items-center gap-3">
        <h2 className="section-title">Structure: List</h2>
        <button
          className="btn btn-sm"
          onClick={doSuggest}
          disabled={suggest.isPending}
        >
          {suggest.isPending ? "Suggesting…" : "Suggest kinds"}
        </button>
      </div>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        A kind groups similar objects, such as stores or suppliers. A link type
        describes how two kinds connect.
      </p>
      {note && <p role="status">{note}</p>}
      {error && <ErrorLine error={error} />}
      <div className="grid gap-3 sm:flex">
        <Search />
        <Filter
          label="Type"
          param="type"
          options={[
            { value: "all", label: "Kinds and links" },
            { value: "kind", label: "Kinds" },
            { value: "link", label: "Link types" },
          ]}
        />
        <Filter label="Status" param="status" options={statusOptions} />
      </div>
      <section aria-label="Structure list" className="min-w-0">
        <div className="ledger overflow-x-auto">
          <table className="data-table w-full">
            <caption className="sr-only">Kinds and link types</caption>
            <thead>
              <tr>
                <th>Definition</th>
                <th>Count</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {rows.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE).map((r) => (
                <tr key={r.id}>
                  <td className="max-w-[38ch] [overflow-wrap:anywhere]">
                    <Link
                      className="link"
                      href={
                        r.type === "kind"
                          ? kindHref(vault.id, r.kind, params.toString())
                          : twinHref("/ontology/review", vault.id, {
                              selected: r.id,
                              status: "all",
                            })
                      }
                    >
                      {r.title}
                    </Link>
                    <span className="label block">
                      {r.type === "kind" ? "Kind" : "Link type"}
                    </span>
                  </td>
                  <td>
                    <span className="font-mono">
                      {r.count.toLocaleString("en-AU")}
                    </span>
                    <span className="hidden sm:inline"> {r.unit}</span>
                  </td>
                  <td>
                    <StatusLabel tone={STATUS[r.status].tone}>
                      {STATUS[r.status].label}
                    </StatusLabel>
                  </td>
                </tr>
              ))}
              {rows.length === 0 && (
                <tr>
                  <td colSpan={3}>
                    {data.kinds.some(visibleKind) || data.links.length > 0
                      ? "No definitions match. Change the search or filters."
                      : "No definitions recorded yet. Use Add data to map your records."}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <Pagination page={page} total={rows.length} />
      </section>
      <section aria-label="Space policy" className="flex flex-col gap-3 pt-3">
        <h2 className="section-title">Space policy</h2>
        <StrictSwitch
          vaultId={vault.id}
          on={data.strict}
          isOwner={vault.role === "owner"}
          onError={setError}
        />
      </section>
    </>
  );
}
