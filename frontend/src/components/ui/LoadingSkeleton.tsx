"use client";

import { lazy, Suspense, type ReactNode } from "react";

const MeasuredSkeleton = lazy(() => import("./MeasuredSkeleton"));

type Layout =
  | "people"
  | "rows"
  | "table"
  | "overview"
  | "detail"
  | "settings"
  | "messages"
  | "graph";

// Samples contain no user data or active controls. Skullet measures the same typography and ledgers as the views.
export default function LoadingSkeleton({
  label = "Loading",
  layout = "rows",
  rows = 3,
  children,
}: {
  label?: string;
  layout?: Layout;
  rows?: number;
  children?: ReactNode;
}) {
  const sample = children ?? <Sample layout={layout} rows={rows} />;
  return (
    <div>
      {label && (
        <span className="sr-only" role="status" aria-live="polite">
          {label}
        </span>
      )}
      <Suspense
        fallback={
          <div
            className="loading-skeleton loading-fallback"
            aria-busy="true"
            aria-hidden="true"
            inert
          >
            {sample}
          </div>
        }
      >
        <MeasuredSkeleton>{sample}</MeasuredSkeleton>
      </Suspense>
    </div>
  );
}

function Sample({ layout, rows }: { layout: Layout; rows: number }) {
  if (layout === "people")
    return (
      <table className="w-full text-[13px] border-collapse">
        <thead>
          <tr>
            <th className="text-left py-2">Name</th>
            <th className="hidden md:table-cell text-left">Latest memory</th>
            <th className="hidden md:table-cell">Memories</th>
            <th>Last updated</th>
          </tr>
        </thead>
        <tbody>
          {Array.from({ length: rows }, (_, i) => (
            <tr key={i}>
              <td className="py-2.5">
                <div className="font-medium text-[14px]">Example person</div>
                <span>Person</span>
              </td>
              <td className="hidden md:table-cell">A recently saved memory</td>
              <td className="hidden md:table-cell font-mono">12</td>
              <td>Just now</td>
            </tr>
          ))}
        </tbody>
      </table>
    );
  if (layout === "table")
    return (
      <div className="ledger overflow-hidden">
        <table className="data-table w-full">
          <thead>
            <tr>
              <th>Definition</th>
              <th>Count</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {Array.from({ length: rows }, (_, i) => (
              <tr key={i}>
                <td>Example definition</td>
                <td className="font-mono">120</td>
                <td>Suggested</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  if (layout === "settings")
    return (
      <div className="flex flex-col gap-5">
        <h2 className="section-title">Account settings</h2>
        <div className="ledger hairline-rows">
          {Array.from({ length: rows }, (_, i) => (
            <div
              key={i}
              className="grid gap-3 md:grid-cols-[180px_1fr] px-5 py-4"
            >
              <span>Setting name</span>
              <span skullet-block="" className="field h-10" />
            </div>
          ))}
        </div>
      </div>
    );
  if (layout === "overview")
    return (
      <div className="flex flex-col gap-6">
        <h2 className="section-title">What is saved</h2>
        <p>3 memory spaces · 120 people and topics · 240 memories</p>
        <div className="grid gap-8 lg:grid-cols-2">
          <Sample layout="rows" rows={rows} />
          <Sample layout="rows" rows={rows} />
        </div>
      </div>
    );
  if (layout === "detail")
    return (
      <div className="ledger p-4 flex flex-col gap-4">
        <h2 className="section-title">Memory details</h2>
        <p>Updated recently in your memory space</p>
        <p>Saved information appears here once it has loaded.</p>
        <Sample layout="rows" rows={rows} />
      </div>
    );
  if (layout === "graph")
    return (
      <div className="relative h-64">
        {[
          [20, 30],
          [70, 12],
          [55, 60],
          [15, 75],
          [82, 70],
        ].map(([x, y]) => (
          <span
            key={x}
            skullet-block=""
            className="absolute w-8 h-8 rounded-full"
            style={{ left: `${x}%`, top: `${y}%` }}
          />
        ))}
      </div>
    );
  return (
    <div
      className={
        layout === "messages" ? "flex flex-col gap-6" : "ledger hairline-rows"
      }
    >
      {Array.from({ length: rows }, (_, i) => (
        <div
          key={i}
          className={
            layout === "messages"
              ? "flex flex-col gap-3 p-4"
              : "flex items-center justify-between gap-4 px-4 py-4"
          }
        >
          <div className="flex flex-col gap-1.5">
            <span className="text-[14px] font-medium">
              {layout === "messages" ? "You, just now" : "Example name"}
            </span>
            <span className="text-[13px]">
              {i % 2
                ? "A recently saved memory"
                : "Details in your memory space"}
            </span>
          </div>
          {layout !== "messages" && <span className="label">Connected</span>}
        </div>
      ))}
    </div>
  );
}

// A div cannot be a child of tbody. Keep the table structure and measure each cell.
export function SkeletonTableRows({
  cols,
  cellClasses = [],
  rows = 2,
  label = "Loading rows",
}: {
  cols: number;
  cellClasses?: readonly string[];
  rows?: number;
  label?: string;
}) {
  return (
    <>
      {Array.from({ length: rows }, (_, i) => (
        <tr key={i}>
          {Array.from({ length: cols }, (_, c) => (
            <td key={c} className={cellClasses[c]}>
              <LoadingSkeleton label={i === 0 && c === 0 ? label : ""}>
                <span className="text-[13px]">
                  {c === 0 ? "Example name" : "Details"}
                </span>
              </LoadingSkeleton>
            </td>
          ))}
        </tr>
      ))}
    </>
  );
}
