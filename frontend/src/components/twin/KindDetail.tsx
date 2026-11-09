"use client";

import Link from "next/link";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import Definition from "./Definition";
import { words } from "./DefinitionRows";
import { Filter, Pagination, useTwinParams } from "./controls";
import { definitions, pageNumber, PAGE_SIZE, twinHref } from "./model";

const sections = [
  { value: "definition", label: "Definition" },
  { value: "properties", label: "Properties" },
  { value: "metrics", label: "Numbers" },
  { value: "links", label: "Incoming and outgoing links" },
  { value: "reviews", label: "Review record" },
];
export default function KindDetail({
  vault,
  data,
  kind,
}: {
  vault: Vault;
  data: OntologyOut;
  kind: string;
}) {
  const { params } = useTwinParams();
  const item = data.kinds.find((k) => k.kind === kind);
  const section = params.get("section") ?? "definition";
  const all = definitions(data);
  const rows = all.filter((d) =>
    section === "definition"
      ? d.type === "kind" && d.kind === kind
      : section === "properties"
        ? d.type === "property" && d.kind === kind
        : section === "metrics"
          ? d.type === "metric" && d.kind === kind
          : section === "links"
            ? d.type === "link" &&
              (d.item.from_kind === kind || d.item.to_kind === kind)
            : false,
  );
  const page = pageNumber(params.get("page"), rows.length);
  if (!item)
    return (
      <>
        <h2 className="section-title">Kind unavailable</h2>
        <Link className="link" href={twinHref("/ontology/schema", vault.id)}>
          Back to Structure
        </Link>
      </>
    );
  const pending = all.filter(
    (d) => d.kind === kind && d.item.status === "proposed",
  ).length;
  return (
    <>
      <Link
        className="link self-start text-[13px]"
        href={
          params.get("back")
            ? `/ontology/schema?${new URLSearchParams(params.get("back") ?? "")}`
            : twinHref("/ontology/schema", vault.id)
        }
      >
        Back to Structure
      </Link>
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <h2 className="text-[22px] font-medium">{words(kind)}</h2>
        <Link className="link text-[13px]" href={twinHref("/ontology/objects", vault.id, { kind })}>
          <span className="font-mono">
            {item.entities.toLocaleString("en-AU")}
          </span>{" "}
          objects
        </Link>
      </div>
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        {item.description || "No definition recorded."}
      </p>
      <div className="flex flex-wrap gap-4 items-end">
        <Filter
          label="Kind section"
          param="section"
          fallback="definition"
          options={sections}
        />
        {pending > 0 && (
          <Link
            className="link"
            href={twinHref("/ontology/review", vault.id, { kind })}
          >
            {pending} suggestions to review
          </Link>
        )}
      </div>
      {section === "reviews" ? (
        <section className="ledger p-4 flex flex-col gap-3">
          <h3 className="section-title">Review record</h3>
          <p>
            The current API stores the definition’s status, but does not provide
            who decided, when, or their reason.
          </p>
          <Link
            className="link self-start"
            href={twinHref("/ontology/review", vault.id, { kind, status: "all" })}
          >
            See current decisions for this kind
          </Link>
        </section>
      ) : (
        <>
          <section
            className="ledger p-4 flex flex-col gap-4"
            aria-label={
              sections.find((s) => s.value === section)?.label ?? "Kind details"
            }
          >
            {rows.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE).map((d) => (
              <details
                key={d.id}
                open={section === "definition" || undefined}
                className="border-b pb-3 last:border-0"
                style={{ borderColor: "var(--border)" }}
              >
                <summary className="cursor-pointer py-2 text-[14px] font-medium">
                  {words(d.name)}
                  {d.type === "property"
                    ? ` (${d.item.datatype}${d.item.unit ? `, ${d.item.unit}` : ""})`
                    : ""}
                  {d.type === "link"
                    ? d.item.from_kind === kind
                      ? " (outgoing)"
                      : " (incoming)"
                    : ""}
                </summary>
                <Definition
                  definition={d}
                  data={data}
                  isOwner={vault.role === "owner"}
                />
              </details>
            ))}
            {rows.length === 0 && (
              <p className="text-[13px]">No definitions in this section yet.</p>
            )}
          </section>
          {section !== "definition" && (
            <Pagination page={page} total={rows.length} />
          )}
        </>
      )}
    </>
  );
}
