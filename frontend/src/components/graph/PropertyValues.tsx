// An entity's typed properties in the inspector: each approved property's current value, with its
// unit and the date it held from. Nothing renders when the entity has none.
import type { PropertyValue } from "@/lib/gen";

const words = (name: string) => name.replace(/_/g, " ");

function shown(p: PropertyValue): string {
  const v = p.value;
  const text = p.datatype === "money" && typeof v === "number" ? v.toFixed(2) : typeof v === "boolean" ? (v ? "yes" : "no") : String(v);
  return p.unit ? `${text} ${p.unit}` : text;
}

export default function PropertyValues({ properties }: { properties: PropertyValue[] | undefined }) {
  if (!properties?.length) return null;
  return (
    <section className="flex flex-col gap-2">
      <h3 className="section-title">Properties</h3>
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-[14px]">
        {properties.map((p) => (
          <div key={p.property} className="contents">
            <dt style={{ color: "var(--ink-dim)" }}>{words(p.property)}</dt>
            <dd className="flex flex-wrap items-baseline gap-x-2 min-w-0">
              <span className="tabular-nums">{shown(p)}</span>
              <span className="font-mono text-[12px]" style={{ color: "var(--ink-faint)" }}>
                {p.as_of.slice(0, 10)}
              </span>
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
