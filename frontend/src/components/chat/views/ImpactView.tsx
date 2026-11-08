import { LightningIcon } from "@phosphor-icons/react/ssr";
import Duo from "@/components/ui/Duo";
import { EntityName, PathSentence, words } from "./bits";
import type { Affected, ImpactView as Impact } from "./types";

const away = (n: number) => (n === 1 ? "Directly linked" : `${n} links away`);

/** What a change to one entity affects: the start, then the affected entities grouped by how many
 * relations away they are, each with the chain that reaches it and the model's reason. */
export default function ImpactView({ view }: { view: Impact }) {
  const groups = new Map<number, Affected[]>();
  for (const a of view.affected) groups.set(a.path.length, [...(groups.get(a.path.length) ?? []), a]);
  const steps = [...groups.keys()].sort((a, b) => a - b);
  return (
    <section className="ledger p-4 flex flex-col gap-3" aria-label="What this affects">
      <header className="flex items-start gap-2.5">
        <Duo icon={LightningIcon} size={18} tone="company" className="mt-0.5" />
        <div className="min-w-0 flex flex-col gap-1">
          <h3 className="section-title">{view.headline}</h3>
          <div className="flex flex-wrap items-center gap-x-1.5 text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Starting from <span className="pill !min-h-7 !px-2.5" style={{ color: "var(--ink)" }}><EntityName entity={view.start} strong /></span>
            <span>{words(view.start.kind)}</span>
          </div>
        </div>
      </header>
      {view.linked === false ? (
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }} data-testid="impact-no-links">
          {view.start.name} has no saved links yet, so there is nothing to follow. Link it to the things it touches, then ask again.
        </p>
      ) : (
        view.affected.length === 0 && (
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Nothing else is affected.
          </p>
        )
      )}
      {steps.map((n) => (
        <div key={n} className="flex flex-col gap-2">
          <h4 className="label">{away(n)}</h4>
          <ul className="grid gap-2 sm:grid-cols-2">
            {groups.get(n)!.map((a) => (
              <li key={a.entity.id} className="soft-panel px-3 py-2.5 flex flex-col gap-1 min-w-0 text-[13px]">
                <div className="flex items-baseline justify-between gap-2 min-w-0 text-[14px]">
                  <EntityName entity={a.entity} strong />
                  <span className="label shrink-0">{words(a.entity.kind)}</span>
                </div>
                <PathSentence path={a.path} />
                <p style={{ color: "var(--ink-dim)" }}>{a.why}</p>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
