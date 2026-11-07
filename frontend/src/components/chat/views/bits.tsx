import Link from "next/link";
import { kindColor } from "@/lib/kindColors";
import type { EntityRef, Step } from "./types";

/** `part_of` reads as "part of". */
export const words = (label: string) => label.replace(/_/g, " ");

export const entityHref = (e: EntityRef) => `/entities?id=${encodeURIComponent(e.id)}&vault=${encodeURIComponent(e.vault)}`;

/** An entity's name, linking to its page, after a dot in its kind's colour. */
export function EntityName({ entity, strong }: { entity: EntityRef; strong?: boolean }) {
  return (
    <Link href={entityHref(entity)} className={`inline-flex items-baseline gap-1.5 min-w-0 hover:underline underline-offset-2 ${strong ? "font-medium" : ""}`}>
      <span className="w-2 h-2 rounded-full shrink-0 self-center" style={{ background: kindColor(entity.kind) }} aria-hidden />
      <span className="truncate">{entity.name}</span>
    </Link>
  );
}

/** "Truck 7 carries Shipment 88, then Shipment 88 fulfils Order 12." */
export function PathSentence({ path }: { path: Step[] }) {
  return (
    <span>
      {path.map((s, i) => (
        <span key={s.id}>
          {i > 0 && ", then "}
          {s.from.name} <span style={{ color: "var(--ink-dim)" }}>{words(s.label)}</span> {s.to.name}
        </span>
      ))}
      .
    </span>
  );
}
