"use client";

import { useOauthGrants } from "@/lib/queries/oauth";
import { useTokens } from "@/lib/queries/auth";
import { confirmedConnections, type Connection } from "./apps";

/** Server-confirmed AI app connections. `null` while loading. */
export function useConnections(): { connections: Connection[] | null; error: boolean; recheck: () => Promise<Connection[]> } {
  const grants = useOauthGrants();
  const tokens = useTokens();
  const loading = (grants.isPending && !grants.isError) || (tokens.isPending && !tokens.isError);
  const connections = loading ? null : confirmedConnections(grants.data, tokens.data);
  async function recheck() {
    const [g, t] = await Promise.all([grants.refetch(), tokens.refetch()]);
    return confirmedConnections(g.data, t.data);
  }
  return { connections, error: grants.isError && tokens.isError, recheck };
}
