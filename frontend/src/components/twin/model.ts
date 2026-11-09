import type {
  KindItem,
  LinkItem,
  MetricItem,
  OntologyOut,
  PropertyItem,
} from "@/lib/queries/ontology";

export type Definition =
  | { type: "kind"; id: string; kind: string; name: string; item: KindItem }
  | {
      type: "property";
      id: string;
      kind: string;
      name: string;
      item: PropertyItem;
    }
  | { type: "metric"; id: string; kind: string; name: string; item: MetricItem }
  | { type: "link"; id: string; kind: string; name: string; item: LinkItem };

export function definitions(data: OntologyOut): Definition[] {
  return [
    ...data.kinds.flatMap((item): Definition[] => [
      {
        type: "kind",
        id: JSON.stringify(["kind", item.kind]),
        kind: item.kind,
        name: item.kind,
        item,
      },
      ...item.properties.map((p): Definition => ({
        type: "property",
        id: JSON.stringify(["property", item.kind, p.name]),
        kind: item.kind,
        name: p.name,
        item: p,
      })),
      ...item.metrics.map((m): Definition => ({
        type: "metric",
        id: JSON.stringify(["metric", item.kind, m.name]),
        kind: item.kind,
        name: m.name,
        item: m,
      })),
    ]),
    ...data.links.map((item): Definition => ({
      type: "link",
      id: JSON.stringify(["link", item.from_kind, item.label, item.to_kind]),
      kind: item.from_kind,
      name: `${item.from_kind} ${item.label} ${item.to_kind}`,
      item,
    })),
  ];
}

export const twinHref = (
  path: string,
  space: string,
  params: Record<string, string> = {},
) => `${path}?${new URLSearchParams({ space, ...params })}`;
export const kindHref = (space: string, kind: string, back = "") =>
  twinHref(`/ontology/kinds/${encodeURIComponent(kind)}`, space, back ? { back } : {});
export const PAGE_SIZE = 10;
export function pageNumber(value: string | null, total: number) {
  return Math.min(
    Math.max(1, Number(value) || 1),
    Math.max(1, Math.ceil(total / PAGE_SIZE)),
  );
}
export const visibleKind = (k: KindItem) =>
  !(
    k.built_in &&
    k.entities === 0 &&
    k.status === "observed" &&
    k.properties.length === 0 &&
    k.metrics.length === 0
  );
