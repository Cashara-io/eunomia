"use client";

import Link from "next/link";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import { ReadinessPanel, STATUS, words } from "./DefinitionRows";
import { definitions, twinHref, visibleKind } from "./model";

export default function Overview({
  vault,
  data,
}: {
  vault: Vault;
  data: OntologyOut;
}) {
  const all = definitions(data);
  const pending = all.filter((d) => d.item.status === "proposed");
  const groups = [
    { label: "Kinds", items: data.kinds.filter(visibleKind) },
    { label: "Properties", items: data.kinds.flatMap((k) => k.properties) },
    { label: "Link types", items: data.links },
    { label: "Metrics", items: data.kinds.flatMap((k) => k.metrics) },
  ];
  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="section-title">{vault.name}: Overview</h2>
        <Link
          className="btn btn-primary"
          href={twinHref("/twin/review", vault.id)}
        >
          {pending.length} to review
        </Link>
      </div>
      <section
        aria-label="Structure by status"
        className="ledger overflow-x-auto"
      >
        <table className="data-table w-full">
          <caption className="sr-only">Definitions by status</caption>
          <thead>
            <tr>
              <th>Definition</th>
              {Object.entries(STATUS).map(([key, s]) => (
                <th key={key}>{s.label}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {groups.map((g) => (
              <tr key={g.label}>
                <th scope="row">{g.label}</th>
                {Object.keys(STATUS).map((s) => (
                  <td key={s} className="font-mono">
                    {g.items
                      .filter((i) => i.status === s)
                      .length.toLocaleString("en-AU")}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </section>
      <section className="flex flex-col gap-3">
        <h2 className="section-title">Ready for numbers</h2>
        <ReadinessPanel vaultId={vault.id} />
      </section>
      <div className="grid gap-6 lg:grid-cols-2">
        <section className="flex flex-col gap-3">
          <h2 className="section-title">Suggestions to review</h2>
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Current suggestions. Proposal dates are not recorded.
          </p>
          <ul className="ledger hairline-rows">
            {pending.slice(0, 5).map((d) => (
              <li key={d.id} className="px-4 py-3">
                <Link
                  className="link"
                  href={twinHref("/twin/review", vault.id, { selected: d.id })}
                >
                  {words(d.name)}
                </Link>
                <span className="label ml-2">{d.type}</span>
              </li>
            ))}
            {pending.length === 0 && (
              <li className="p-4 text-[13px]">
                Nothing is waiting for review.
              </li>
            )}
          </ul>
          {pending.length > 5 && (
            <Link className="link" href={twinHref("/twin/review", vault.id)}>
              See all {pending.length} suggestions
            </Link>
          )}
        </section>
        <section className="flex flex-col gap-3">
          <h2 className="section-title">Decisions</h2>
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Approved and rejected definitions are available in Structure. The
            current API does not provide dated decisions or a review history.
          </p>
          <Link
            className="link self-start"
            href={twinHref("/twin/schema", vault.id)}
          >
            Browse the structure
          </Link>
        </section>
      </div>
      {data.kinds.filter(visibleKind).length === 0 && (
        <p>
          No business kinds yet. Use Add data to map records, or ask your AI app
          to suggest a structure.
        </p>
      )}
    </>
  );
}
