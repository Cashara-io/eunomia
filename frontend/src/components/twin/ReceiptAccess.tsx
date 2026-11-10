"use client";
import type { ReactNode } from "react";
import SpaceAccess, { type ProtectedQueries } from "./SpaceAccess";
const queries = { keys: ["object-profile", "objects", "object-kinds", "ontology-activity"], objectKeys: ["object-profile"], checkOnMount: true } satisfies ProtectedQueries;
export default function ReceiptAccess({ space, children }: { space: string; children: ReactNode }) {
  return <SpaceAccess space={space} queries={queries}>{children}</SpaceAccess>;
}
