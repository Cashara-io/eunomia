"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import {
  DotsThreeIcon,
  ArrowClockwiseIcon,
  TreeStructureIcon,
} from "@phosphor-icons/react";
import styles from "./overview.module.css";
import OverviewSkeleton from "./OverviewSkeleton";
import MorphText from "@/components/ui/MorphText";
import ReadState from "./ReadState";
import { returnKey } from "./overview-model";

import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { usePathname, useSearchParams } from "next/navigation";
import { useVaults } from "@/lib/queries/vaults";
import { useOntology, useReadiness } from "@/lib/queries/ontology";
import type { Vault } from "@/lib/types";
import VaultChip, { vaultTone } from "@/components/VaultChip";
import AutomapBar from "@/components/vaults/AutomapBar";
import { definitions, twinHref } from "./model";
import Activity from "./Activity";
import Overview from "./Overview";
import { OverviewHighlight } from "./OverviewHighlight";
import Structure from "./Structure";
import KindDetail from "./KindDetail";
import Review from "./Review";
import ReviewSkeleton from "./ReviewSkeleton";
import Objects from "./Objects";
import Connections from "./Connections";
import ConnectionAccess, { ConnectionContent } from "./ConnectionAccess";
import ObjectProfile from "./ObjectProfile";

const views = [
  { path: "/ontology", label: "Overview" },
  { path: "/ontology/connections", label: "Connections" },
  { path: "/ontology/objects", label: "Objects" },
  { path: "/ontology/review", label: "Review" },
];
const moreViews = [
  { path: "/ontology/schema", label: "Structure" },
  { path: "/ontology/activity", label: "Activity" },
];

export default function TwinArea({
  view,
  kind,
}: {
  view:
    | "connections"
    | "overview"
    | "structure"
    | "review"
    | "kind"
    | "objects"
    | "object"
    | "activity";
  kind?: string;
}) {
  const query = useVaults();
  const params = useSearchParams();
  const space = params.get("space");
  const vault = query.data?.find((v) => v.id === space);
  const cache = useQueryClient();
  const previousConnectionSpace = useRef<string | undefined>(undefined);
  const connectionSpace = view === "connections" ? vault?.id : undefined;
  useEffect(() => {
    const previous = previousConnectionSpace.current;
    previousConnectionSpace.current = connectionSpace;
    if (!previous || previous === connectionSpace) return;
    if (view === "connections" && !connectionSpace && new URLSearchParams(window.location.search).get("space") === previous) {
      window.history.replaceState(null, "", `${window.location.pathname}?${new URLSearchParams({ space: previous })}`);
    }
    const scoped = { predicate: (q: { queryKey: readonly unknown[] }) => q.queryKey.includes(previous) };
    void cache.cancelQueries(scoped);
    cache.removeQueries(scoped);
  }, [cache, connectionSpace, view]);
  return (
    <div className={styles.area}>
      {query.isError ? (
        <ReadState
          error={query.error}
          retry={() => void query.refetch()}
          subject="your memory spaces"
        />
      ) : !query.data ? (
        <LoadingSkeleton label="Loading memory spaces" />
      ) : vault ? (
        view === "connections" ? (
          <ConnectionAccess key={vault.id} space={vault.id}>
            <SelectedSpace vault={vault} view={view} kind={kind} />
          </ConnectionAccess>
        ) : <SelectedSpace key={vault.id} vault={vault} view={view} kind={kind} />
      ) : (
        <>
          <h1 className="page-title">Ontology</h1>
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
                  view === "connections"
                    ? "/ontology/connections"
                    : view === "activity"
                      ? "/ontology/activity"
                      : view === "review"
                        ? "/ontology/review"
                        : view === "objects" || view === "object"
                          ? "/ontology/objects"
                          : view === "structure"
                            ? "/ontology/schema"
                            : "/ontology",
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
  view:
    | "connections"
    | "overview"
    | "structure"
    | "review"
    | "kind"
    | "objects"
    | "object"
    | "activity";
  kind?: string;
}) {
  const objectView = view === "objects" || view === "object";
  const query = useOntology(vault.id, !objectView);
  const cache = useQueryClient();
  const readiness = useReadiness(vault.id, !objectView);
  const [adding, setAdding] = useState(false);
  const pathname = usePathname();
  const params = useSearchParams();
  useEffect(() => {
    if (view !== "object") return;
    try {
      sessionStorage.setItem(
        returnKey(vault.id),
        pathname + "?" + params.toString(),
      );
    } catch {
      /* Storage may be disabled. */
    }
  }, [view, vault.id, pathname, params]);
  const data = query.data;
  const waiting = data
    ? definitions(data).filter((d) => d.item.status === "proposed").length
    : null;
  const current =
    view === "kind"
      ? "/ontology/schema"
      : view === "object"
        ? "/ontology/objects"
        : pathname;
  return (
    <>
      <ConnectionContent>
      <header className={styles.context}>
        <div className={styles.identity}>
          <Link href="/ontology" className={styles.areaName}>
            <TreeStructureIcon size={18} weight="duotone" aria-hidden />
            Ontology
          </Link>
          <Link
            href="/ontology"
            className={styles.spaceLink}
            aria-label={`Change space, current space ${vault.name}`}
          >
            <VaultChip tone={vaultTone(vault.kind)} name={vault.name} />
          </Link>
        </div>
        <div className={styles.contextActions}>
          <button
            className="btn btn-ghost btn-sm"
            aria-label="Refresh ontology"
            disabled={query.isFetching}
            onClick={() => {
              if (!objectView) {
                void query.refetch();
                void readiness.refetch();
              }
              for (const key of [
                "object-kinds",
                "objects",
                "object-profile",
                "connections",
                "connection-search",
                "ontology-activity",
              ])
                void cache.invalidateQueries({ queryKey: [key, vault.id] });
            }}
          >
            <ArrowClockwiseIcon size={16} aria-hidden />
            <span>Refresh</span>
          </button>
          {vault.role === "owner" && (
            <button
              className="btn btn-sm"
              aria-expanded={adding}
              aria-controls="ontology-add-data"
              onClick={() => setAdding(!adding)}
            >
              Add data
            </button>
          )}
        </div>
      </header>
      <nav aria-label="Ontology views" className={styles.navigation}>
        {views.map((v) => (
          <Link
            key={v.path}
            className={styles.navLink}
            aria-current={current === v.path ? "page" : undefined}
            href={twinHref(v.path, vault.id)}
          >
            {v.label}
            {v.label === "Review" && waiting !== null && waiting > 0 && (
              <MorphText className={styles.navCount}>
                {waiting.toLocaleString("en-AU")}
              </MorphText>
            )}
          </Link>
        ))}
        <details
          className={styles.more}
          key={pathname}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.currentTarget.open = false;
              e.currentTarget.querySelector("summary")?.focus();
            }
          }}
        >
          <summary
            className={styles.navLink}
            aria-current={
              moreViews.some((v) => v.path === current) ? "page" : undefined
            }
          >
            More <DotsThreeIcon aria-hidden size={18} />
          </summary>
          <div className={styles.morePanel}>
            {moreViews.map((v) => (
              <Link
                key={v.path}
                aria-current={v.path === current ? "page" : undefined}
                href={twinHref(v.path, vault.id)}
              >
                {v.label}
              </Link>
            ))}
          </div>
        </details>
      </nav>
      {adding && (
        <section
          id="ontology-add-data"
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
      </ConnectionContent>
      {view !== "overview" && (
        <h1 className="sr-only">
          Ontology:{" "}
          {view === "kind" ? kind : view === "object" ? "Object" : view}
        </h1>
      )}
      {view === "connections" ? (
        <Connections space={vault.id} />
      ) : view === "activity" ? (
        <Activity space={vault.id} />
      ) : view === "objects" ? (
        <Objects space={vault.id} />
      ) : view === "object" ? (
        <ObjectProfile key={kind} space={vault.id} id={kind ?? ""} />
      ) : query.isError ? (
        <ReadState
          error={query.error}
          retry={() => void query.refetch()}
          subject="the ontology"
        />
      ) : !data ? (
        view === "overview" ? (
          <OverviewSkeleton />
        ) : view === "review" ? <ReviewSkeleton /> : (
          <LoadingSkeleton
            label="Loading the ontology"
            layout="table"
            rows={5}
          />
        )
      ) : (
        <>
          {view === "overview" && (
            <OverviewHighlight>
              <Overview
                vault={vault}
                data={data}
                onAdd={() => setAdding(true)}
              />
              <Activity space={vault.id} compact />
            </OverviewHighlight>
          )}
          {view === "structure" && <Structure vault={vault} data={data} />}
          {view === "kind" && (
            <KindDetail vault={vault} data={data} kind={kind ?? ""} />
          )}
          {view === "review" && <Review vault={vault} data={data} />}
          {view === "review" && <Activity space={vault.id} compact />}
        </>
      )}
    </>
  );
}
