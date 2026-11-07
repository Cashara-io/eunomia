"use client";

// The one memory-space picker for People & topics (List, Connections, Memory map) and Code map.
// `undefined` is Personal, as the graph endpoint reads it.

import Select from "@/components/Select";
import { useVaults } from "@/lib/queries/vaults";
import type { Vault } from "@/lib/types";

// The colour a space wears everywhere (its dot on Memory spaces, its points on the Memory map).
export { spaceColor } from "@/components/vaults/bits";

export const spaceName = (v: Pick<Vault, "kind" | "name">) => (v.kind === "personal" ? "Personal" : v.name);

export default function SpaceSelect({
  value,
  onChange,
  className = "h-10 text-[14px] w-56",
}: {
  value: string | undefined;
  onChange: (id: string | undefined) => void;
  className?: string;
}) {
  const vaults = useVaults().data ?? [];
  if (vaults.length < 2) return null;
  return (
    <Select
      className={className}
      value={value ?? ""}
      onChange={(v) => onChange(v || undefined)}
      aria-label="Memory space"
      options={vaults.map((v) => ({ value: v.kind === "personal" ? "" : v.id, label: spaceName(v) }))}
    />
  );
}
