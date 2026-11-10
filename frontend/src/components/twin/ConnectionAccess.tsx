import type { ReactNode } from "react";
import SpaceAccess, { type ProtectedQueries } from "./SpaceAccess";
export { ProtectedSpaceContent as ConnectionContent, useConfirmedSpaceAccess as useConfirmedConnectionAccess, useMissingSpaceObjects as useMissingConnectionObjects } from "./SpaceAccess";
const queries = { keys: ["connections", "connection-search"], objectKeys: ["object-profile", "connections"] } satisfies ProtectedQueries;
export default function ConnectionAccess({ space, children }: { space: string; children: ReactNode }) {
  return <SpaceAccess space={space} queries={queries}>{children}</SpaceAccess>;
}
