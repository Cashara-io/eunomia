import { GraphIcon } from "@phosphor-icons/react/ssr";
import Duo from "@/components/ui/Duo";
import { EntityName, words } from "./bits";
import type { SubgraphView as Subgraph } from "./types";

/** How some entities connect: the entities as pills, then each relation as a sentence. */
export default function SubgraphView({ view }: { view: Subgraph }) {
  return (
    <section className="ledger p-4 flex flex-col gap-3" aria-label={view.title}>
      <header className="flex items-start gap-2.5">
        <Duo icon={GraphIcon} size={18} tone="project" className="mt-0.5" />
        <h3 className="section-title min-w-0">{view.title}</h3>
      </header>
      <ul className="flex flex-wrap gap-1.5" aria-label="Entities">
        {view.entities.map((e) => (
          <li key={e.id} className="pill !min-h-7 !px-2.5 max-w-full">
            <EntityName entity={e} />
          </li>
        ))}
      </ul>
      {view.relations.length > 0 && (
        <ul className="hairline-rows text-[13px]" aria-label="How they connect">
          {view.relations.map((r) => (
            <li key={r.id} className="flex flex-wrap items-baseline gap-x-1.5 py-1.5 min-w-0">
              <EntityName entity={r.from} />
              <span style={{ color: "var(--ink-dim)" }}>{words(r.label)}</span>
              <EntityName entity={r.to} />
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
