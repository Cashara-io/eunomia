"use client";
import { useCallback, type CSSProperties } from "react";
import { useKindColor } from "@/lib/kindColors";
// Seven familiar palette families connect the plane, icons and linked panels.
export const families: Record<string, string> = {
  customer: "person",
  employee: "person",
  supplier: "person",
  product: "repository",
  stock_item: "repository",
  order: "company",
  purchase_order: "company",
  sale: "company",
  invoice: "file",
  payment: "file",
  contract: "symbol",
  delivery: "organisation",
  shipment: "organisation",
  delivery_route: "organisation",
  warehouse: "location",
};
export function useLandscapeTint() {
  const color = useKindColor();
  const hue = useCallback(
    (kind: string) =>
      families[kind] === "company"
        ? "var(--vault-company)"
        : color(families[kind] ?? kind),
    [color],
  );
  const tint = useCallback(
    (kind: string) => ({ "--kind-hue": hue(kind) }) as CSSProperties,
    [hue],
  );
  return { hue, tint };
}
