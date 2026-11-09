"use client";

import { Skeleton, SkulletProvider } from "skullet/react";
import type { ReactNode } from "react";

export default function MeasuredSkeleton({
  children,
}: {
  children: ReactNode;
}) {
  return (
    <SkulletProvider
      reveal="together"
      theme={{
        light: {
          base: "var(--surface-raised)",
          highlight: "var(--surface-hover)",
        },
        dark: {
          base: "var(--surface-raised)",
          highlight: "var(--surface-hover)",
        },
        radius: "5px",
        animation: "pulse",
        speed: "slow",
      }}
    >
      <Skeleton loading className="loading-skeleton">
        {children}
      </Skeleton>
    </SkulletProvider>
  );
}
