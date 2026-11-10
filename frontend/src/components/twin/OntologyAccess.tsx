"use client";
import { useState, type ReactNode } from "react";
import { usePathname, useSearchParams } from "next/navigation";
import { useVaults } from "@/lib/queries/vaults";
import SpaceAccess, { type ProtectedQueries } from "./SpaceAccess";

const keys = ["ontology", "object-profile", "objects", "object-kinds", "ontology-activity", "connections", "connection-search"];
export default function OntologyAccess({ children }: { children: ReactNode }) {
  // Membership updates must remain observable while denied pages are unmounted.
  useVaults();
  const space = useSearchParams().get("space");
  const pathname = usePathname();
  return space ? <SpaceGate key={space} space={space} pathname={pathname}>{children}</SpaceGate> : children;
}
function SpaceGate({ space, pathname, children }: { space: string; pathname: string; children: ReactNode }) {
  // Keep the initial policy stable as pages change. Healthy Connections reads
  // authorise themselves, without adding a membership request on navigation.
  const [queries] = useState<ProtectedQueries>(() => ({
    keys,
    objectKeys: ["object-profile", "connections"],
    checkOnMount: pathname.startsWith("/ontology/objects") || pathname === "/ontology/activity",
  }));
  return <SpaceAccess space={space} queries={queries}>{children}</SpaceAccess>;
}
