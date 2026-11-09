"use client";

import Link from "next/link";
import { useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useVaults } from "@/lib/queries/vaults";
import { useOntology, useReadiness } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import ErrorLine, { failure } from "@/components/ErrorLine";
import PageHeader from "@/components/ui/PageHeader";
import VaultChip, { vaultTone } from "@/components/VaultChip";
import Select from "@/components/Select";
import AutomapBar from "@/components/vaults/AutomapBar";
import { definitions, twinHref } from "./model";
import Overview from "./Overview";
import Structure from "./Structure";
import KindDetail from "./KindDetail";
import Review from "./Review";

const views = [
  { path: "/twin", label: "Overview" },
  { path: "/twin/schema", label: "Structure" },
  { path: "/twin/review", label: "Review" },
];

export default function TwinArea({
  view,
  kind,
}: {
  view: "overview" | "structure" | "review" | "kind";
  kind?: string;
}) {
  const query = useVaults();
  const params = useSearchParams();
  const space = params.get("space");
  const vault = query.data?.find((v) => v.id === space);
  return (
    <div className="max-w-6xl flex flex-col gap-6">
      <PageHeader
        title="Business twin"
        intro="The people, work and numbers your business runs on."
      />
      {query.isError ? (
        <ErrorLine
          error={failure(
            query.error,
            "Could not load your memory spaces. Reload to try again.",
          )}
        />
      ) : !query.data ? (
        <p role="status">Loading memory spaces…</p>
      ) : vault ? (
        <SelectedSpace key={vault.id} vault={vault} view={view} kind={kind} />
      ) : (
        <>
          <h2 className="section-title">Choose a memory space</h2>
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            {space
              ? "This space is unavailable. Choose a space you can access."
              : "Each space has its own kinds, links and review decisions."}
          </p>
          <div className="ledger hairline-rows">
            {query.data.map((v) => (
              <Link
                key={v.id}
                href={twinHref(
                  view === "review"
                    ? "/twin/review"
                    : view === "structure"
                      ? "/twin/schema"
                      : "/twin",
                  v.id,
                )}
                className="flex items-center justify-between gap-3 p-4 hover:bg-[var(--surface-raised)]"
              >
                <VaultChip tone={vaultTone(v.kind)} name={v.name} />
                <span className="text-[13px]">
                  {v.role === "owner" ? "Owner" : "Member"}
                </span>
              </Link>
            ))}
          </div>
          {query.data.length === 0 && (
            <Link className="link" href="/vaults">
              Create a memory space to start
            </Link>
          )}
        </>
      )}
    </div>
  );
}

function SelectedSpace({
  vault,
  view,
  kind,
}: {
  vault: Vault;
  view: "overview" | "structure" | "review" | "kind";
  kind?: string;
}) {
  const query = useOntology(vault.id);
  const readiness = useReadiness(vault.id);
  const [adding, setAdding] = useState(false);
  const router = useRouter();
  const pathname = usePathname();
  const data = query.data;
  const waiting = data
    ? definitions(data).filter((d) => d.item.status === "proposed").length
    : null;
  const current = view === "kind" ? "/twin/schema" : pathname;
  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3 min-w-0">
          <VaultChip tone={vaultTone(vault.kind)} name={vault.name} />
          <Link className="link text-[13px]" href="/twin">
            Change space
          </Link>
        </div>
        <div className="flex gap-2">
          <button
            className="btn btn-sm"
            disabled={query.isFetching}
            onClick={() => {
              void query.refetch();
              void readiness.refetch();
            }}
          >
            {query.isFetching ? "Refreshing…" : "Refresh"}
          </button>
          {vault.role === "owner" && (
            <button
              className="btn btn-sm"
              aria-expanded={adding}
              onClick={() => setAdding(!adding)}
            >
              Add data
            </button>
          )}
        </div>
      </div>
      <nav aria-label="Business twin views" className="hidden sm:flex gap-2">
        {views.map((v) => (
          <Link
            key={v.path}
            className="pill"
            aria-current={current === v.path ? "page" : undefined}
            href={twinHref(v.path, vault.id)}
          >
            {v.label}
            {v.label === "Review" && waiting !== null ? ` (${waiting})` : ""}
          </Link>
        ))}
      </nav>
      <div className="sm:hidden flex flex-col gap-1">
        <span className="label">View</span>
        <Select
          aria-label="View"
          value={current}
          options={views.map((v) => ({ value: v.path, label: v.label }))}
          onChange={(v) => router.push(twinHref(v, vault.id))}
          className="w-full min-h-11"
        />
      </div>
      {adding && (
        <section
          aria-label="Add data"
          className="ledger p-4 flex flex-col gap-4"
        >
          <h2 className="section-title">Add data</h2>
          <AutomapBar vaultId={vault.id} />
          <p className="text-[13px]">
            Auto-map suggests definitions. It does not import rows. Review the
            suggestions, then preview and import your CSV in Documents.
          </p>
          <Link
            className="link self-start"
            href={`/documents?space=${encodeURIComponent(vault.id)}`}
          >
            Open Documents to upload or import
          </Link>
        </section>
      )}
      {query.isError ? (
        <ErrorLine
          error={failure(
            query.error,
            "Could not load the business twin. Use Refresh to try again.",
          )}
        />
      ) : !data ? (
        <p role="status">Loading the business twin…</p>
      ) : (
        <>
          {view === "overview" && <Overview vault={vault} data={data} />}
          {view === "structure" && <Structure vault={vault} data={data} />}
          {view === "kind" && (
            <KindDetail vault={vault} data={data} kind={kind ?? ""} />
          )}
          {view === "review" && <Review vault={vault} data={data} />}
        </>
      )}
    </>
  );
}
