import type { OntologyOut } from "@/lib/queries/ontology";
import { visibleKind } from "./model";

export function businessSummary(data: OntologyOut) {
  const kinds = data.kinds.filter(visibleKind);
  const top = [...kinds]
    .sort((a, b) => b.entities - a.entities || a.kind.localeCompare(b.kind))
    .slice(0, 4);
  const names = new Set(top.map((k) => k.kind));
  const pairs = new Set<string>();
  const links = data.links
    .filter(
      (l) =>
        l.status !== "rejected" &&
        l.count > 0 &&
        names.has(l.from_kind) &&
        names.has(l.to_kind),
    )
    .sort(
      (a, b) =>
        b.count - a.count ||
        `${a.from_kind}/${a.label}/${a.to_kind}`.localeCompare(
          `${b.from_kind}/${b.label}/${b.to_kind}`,
        ),
    )
    .filter((l) => {
      const pair = JSON.stringify([l.from_kind, l.to_kind].sort());
      if (pairs.has(pair)) return false;
      pairs.add(pair);
      return true;
    })
    .slice(0, 3);
  return {
    kinds,
    top,
    links,
    total: kinds.reduce((n, k) => n + k.entities, 0),
  };
}

// Store only a local route, never protected names or object values. Reads reauthorise it.
export const returnKey = (space: string) => `ontology-return:${space}`;
export function objectReturn(space: string): string | null {
  try {
    const value = sessionStorage.getItem(returnKey(space));
    if (!value) return null;
    const url = new URL(value, window.location.origin);
    if (
      url.origin !== window.location.origin ||
      !/^\/ontology\/objects\/[^/]+$/.test(url.pathname) ||
      url.searchParams.get("space") !== space
    )
      return null;
    decodeURIComponent(url.pathname.split("/").at(-1) ?? "");
    return url.pathname + url.search;
  } catch {
    return null;
  }
}
