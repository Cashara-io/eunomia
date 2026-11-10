"use client";
import { useMemo, useState } from "react";
import Link from "next/link";
import {
  ArrowRightIcon,
  ClockCounterClockwiseIcon,
  WarningCircleIcon,
  ArrowUpRightIcon,
  PlusIcon,
} from "@phosphor-icons/react";
import type { OntologyOut } from "@/lib/queries/ontology";
import { useObjects, useObjectProfile } from "@/lib/queries/objects";
import type { Vault } from "@/lib/types";
import { ReadinessPanel, STATUS } from "./DefinitionRows";
import { definitions, twinHref, visibleKind } from "./model";
import { businessSummary, objectReturn } from "./overview-model";
import KindSummary, { type LandscapePin } from "./KindSummary";
import { humanLabel, plural } from "./overview-format";
import ReadState from "./ReadState";
import styles from "./overview.module.css";

export default function Overview({
  vault,
  data,
  onAdd,
}: {
  vault: Vault;
  data: OntologyOut;
  onAdd: () => void;
}) {
  const pending = useMemo(
    () => definitions(data).filter((d) => d.item.status === "proposed"),
    [data],
  );
  const { kinds, total } = businessSummary(data);
  const [resume] = useState(() => objectReturn(vault.id));
  const resumeId = resume
    ? decodeURIComponent(resume.split("?")[0].split("/").at(-1) ?? "")
    : "";
  const firstKind =
    [...kinds]
      .filter((k) => k.entities > 0)
      .sort((a, b) => a.kind.localeCompare(b.kind))[0]?.kind ?? "";
  const list = useObjects(
    vault.id,
    new URLSearchParams(),
    resumeId ? "" : firstKind,
  );
  const row = list.data?.rows[0];
  const profile = useObjectProfile(vault.id, resumeId || row?.id || "");
  const object = profile.isError ? undefined : profile.data;
  const stale = useMemo(
    () => object?.properties.filter((p) => p.state === "stale") ?? [],
    [object],
  );
  const pins: LandscapePin[] = useMemo(
    () => [
      ...pending.map((d) => ({
        kind: d.kind,
        label: `Review ${humanLabel(d.name)}`,
        href: twinHref("/ontology/review", vault.id, { selected: d.id }),
        type: "proposal" as const,
      })),
      ...(object && stale.length
        ? [
            {
              kind: object.kind,
              label: `Inspect ${humanLabel(stale[0].name)} on ${object.name}. The source changed after this value was saved. Check the value.`,
              href: twinHref(
                `/ontology/objects/${encodeURIComponent(object.id)}`,
                vault.id,
                { section: "Properties" },
              ),
              type: "stale" as const,
              count: stale.length,
            },
          ]
        : []),
    ],
    [pending, object, stale, vault.id],
  );
  const groups = [
    { label: "Kinds", items: data.kinds.filter(visibleKind) },
    { label: "Properties", items: data.kinds.flatMap((k) => k.properties) },
    { label: "Link types", items: data.links },
    { label: "Metrics", items: data.kinds.flatMap((k) => k.metrics) },
  ];
  return (
    <div className={styles.overview}>
      <header className={styles.headline}>
        <div>
          <h1>
            {kinds.length
              ? "Your business, connected."
              : "Start with the things your business knows."}
          </h1>
          <p>
            {kinds.length ? (
              <>
                <span className="font-mono">
                  {total.toLocaleString("en-AU")}
                </span>{" "}
                {plural("object", total)} · {kinds.length}{" "}
                {plural("kind", kinds.length)} in {vault.name}
              </>
            ) : (
              "Map your records into kinds and relationships. Then review the suggestions."
            )}
          </p>
        </div>
        {pending.length > 0 && (
          <Link
            className="btn btn-primary"
            href={twinHref("/ontology/review", vault.id)}
          >
            Review {pending.length} {plural("suggestion", pending.length)}{" "}
            <ArrowRightIcon size={16} aria-hidden />
          </Link>
        )}
      </header>
      {kinds.length === 0 ? (
        <section className={styles.firstRun} aria-label="No business kinds yet">
          <div className={styles.emptyMark} aria-hidden>
            <PlusIcon size={32} weight="duotone" />
          </div>
          <h2>Give your records a shared language.</h2>
          <p>
            A product, its supplier, an order. Add a CSV or connected records to
            start mapping how they fit together.
          </p>
          {vault.role === "owner" ? (
            <button className="btn btn-primary" onClick={onAdd}>
              Add data <ArrowRightIcon aria-hidden size={16} />
            </button>
          ) : (
            <p>Ask a space owner to add data.</p>
          )}
          <p className={styles.caption}>
            Mapping suggests definitions. Review approves them. Import brings in
            the rows.
          </p>
        </section>
      ) : (
        <>
          <div className={styles.hero}>
            <KindSummary data={data} space={vault.id} pins={pins} />
            <section
              aria-labelledby="judgement-title"
              className={styles.judgement}
            >
              <div className={styles.sectionHead}>
                <h2 id="judgement-title">Needs your judgement</h2>
                <WarningCircleIcon aria-hidden size={20} weight="duotone" />
              </div>
              {pending.slice(0, 2).map((d) => (
                <article className={styles.issue} key={d.id}>
                  <h3>{humanLabel(d.name)}</h3>
                  <p>
                    {d.type === "property"
                      ? `${d.item.values.toLocaleString("en-AU")} recorded values await a definition.`
                      : d.type === "metric"
                        ? "Proposed calculation awaits approval before use."
                        : d.type === "link"
                          ? "Proposed relationship awaits review."
                          : "Proposed kind awaits review."}{" "}
                    {vault.role === "owner"
                      ? "Read the evidence to decide."
                      : "Read the evidence; a space owner decides."}
                  </p>
                  <Link
                    className="link"
                    href={twinHref("/ontology/review", vault.id, {
                      selected: d.id,
                    })}
                  >
                    Read {humanLabel(d.name)}{" "}
                    <ArrowRightIcon aria-hidden size={14} />
                  </Link>
                </article>
              ))}
              {pending.length === 0 && (
                <article className={styles.issue}>
                  <h3>Nothing waiting for review</h3>
                  <p>New suggestions will appear here when they arrive.</p>
                </article>
              )}
              {stale.length > 0 && object ? (
                <article className={`${styles.issue} ${styles.stale}`}>
                  <span className={styles.tag}>Stale value</span>
                  <h3>
                    {object.name}: {humanLabel(stale[0].name)}
                  </h3>
                  <p>
                    The source changed after this value was saved. Check the
                    value.
                  </p>
                  <Link
                    className="link"
                    href={twinHref(
                      `/ontology/objects/${encodeURIComponent(object.id)}`,
                      vault.id,
                      { section: "Properties" },
                    )}
                  >
                    Inspect stale value <ArrowRightIcon aria-hidden size={14} />
                  </Link>
                </article>
              ) : (
                <p className={styles.caption}>
                  {object
                    ? `No stale values reported for ${object.name}.`
                    : "Source checks appear when an object is available."}
                </p>
              )}
              <p className={styles.scope}>
                Source check covers {object ? object.name : "one object"}, not
                the whole space.
              </p>
            </section>
          </div>
          <section className={styles.resume} aria-labelledby="resume-title">
            <div className={styles.sectionHead}>
              <h2 id="resume-title">
                <ClockCounterClockwiseIcon aria-hidden size={20} />{" "}
                {resume ? "Pick up where you left off" : "Explore an object"}
              </h2>
              {resume && (
                <span className={styles.caption}>
                  Last object opened in this tab
                </span>
              )}
            </div>
            {list.isError ? (
              <ReadState
                error={list.error}
                retry={() => void list.refetch()}
                subject="an object to explore"
              />
            ) : profile.isError ? (
              <ReadState
                error={profile.error}
                retry={() => void profile.refetch()}
                subject="your object"
              />
            ) : object ? (
              <Link
                className={styles.objectRow}
                href={
                  resume ||
                  twinHref(
                    `/ontology/objects/${encodeURIComponent(object.id)}`,
                    vault.id,
                  )
                }
              >
                <div>
                  <span className={styles.caption}>
                    {resume ? "Continue reading" : "A place to start"}
                  </span>
                  <h3>{object.name}</h3>
                  <span className={styles.caption}>
                    {humanLabel(object.kind)} · {object.sources.length}{" "}
                    {plural("source", object.sources.length)} returned
                  </span>
                </div>
                <span
                  className={stale.length ? styles.staleWord : styles.caption}
                >
                  {stale.length
                    ? `${humanLabel(stale[0].name)}: Source changed after this value was saved. Check the value.`
                    : "Inspect values and sources"}
                </span>
                <ArrowUpRightIcon size={22} aria-hidden />
              </Link>
            ) : list.isPending && !firstKind ? (
              <p className={styles.caption}>
                No objects recorded yet. Add records to an existing kind.
              </p>
            ) : list.isPending || profile.isFetching ? (
              <div
                role="status"
                aria-label="Loading an object"
                className={styles.bone}
                style={{ height: "5rem" }}
              />
            ) : (
              <p className={styles.caption}>
                No objects recorded in {humanLabel(firstKind)} yet.{" "}
                <Link
                  className="link"
                  href={twinHref("/ontology/objects", vault.id)}
                >
                  Browse other kinds
                </Link>
              </p>
            )}
          </section>
        </>
      )}
      <details className={styles.detail}>
        <summary>
          Structure counts and readiness rule{" "}
          <span className={styles.caption}>
            Definition approval is not value verification
          </span>
        </summary>
        <div className={styles.detailBody}>
          <div
            role="region"
            aria-label="Structure by status"
            tabIndex={0}
            className={styles.tableRegion}
          >
            <table className="data-table w-full">
              <caption className="sr-only">Definitions by status</caption>
              <thead>
                <tr>
                  <th>Definition</th>
                  {Object.entries(STATUS).map(([key, s]) => (
                    <th key={key}>
                      {key === "observed" ? "Used, not reviewed" : s.label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {groups.map((g) => (
                  <tr key={g.label}>
                    <th scope="row">{g.label}</th>
                    {Object.keys(STATUS).map((s) => (
                      <td key={s} className="font-mono">
                        {g.items.filter((i) => i.status === s).length}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <h2>Ready for numbers</h2>
          <ReadinessPanel vaultId={vault.id} />
        </div>
      </details>
    </div>
  );
}
