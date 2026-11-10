import { useId } from "react";
import type { OntologyOut } from "@/lib/queries/ontology";
import type { Definition } from "./model";
import LandscapeSolid from "./LandscapeSolid";
import { useLandscapeTint } from "./landscape-shared";
import { reviewContext } from "./review-visual-model";
import { humanLabel } from "./review-format";
import { STATUS } from "./DefinitionRows";
import landscapeStyles from "./overview.module.css";
import styles from "./review.module.css";

export default function ReviewContext({ definition: d, data, sectionId }: { definition: Definition; data: OntologyOut; sectionId?: string }) {
  const { kinds, links, total } = reviewContext(d, data);
  const { tint } = useLandscapeTint();
  const id = useId();
  return <figure id={sectionId} className={styles.contextPicture} aria-label="Where this definition sits">
    <figcaption><strong>In your business</strong><span>Local context · Focus raised, heights are not counts</span></figcaption>
    <svg viewBox={`0 0 ${Math.max(1, kinds.length) * 200} 140`} aria-hidden="true" className={styles.contextPlane}>
      <defs>
        <pattern id={`${id}-grid`} width="32" height="24" patternUnits="userSpaceOnUse" patternTransform="skewX(-28)">
          <path d="M32 0H0V24" fill="none" stroke="var(--border-strong)" strokeWidth=".6" />
        </pattern>
        <marker id={`${id}-arrow`} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M1 1L7 4 1 7" fill="none" stroke="var(--ink-dim)" /></marker>
      </defs>
      <path d={`M5 60L${kinds.length * 200 - 55} 32L${kinds.length * 200 - 5} 117L55 140Z`} fill={`url(#${id}-grid)`} />
      {links.map((l, i) => {
        const from = kinds.findIndex(k => k.kind === l.from_kind);
        const to = kinds.findIndex(k => k.kind === l.to_kind);
        if (from < 0 || to < 0) return null;
        return <path key={i} data-context-link={l.label} d={from === to ? `M${from * 200 + 150} 90c90 0 90 -100 -30 -80` : `M${from * 200 + 150} 94Q${(from + to) * 100 + 100} 130 ${to * 200 + 38} 94`} fill="none" stroke="var(--ink-dim)" strokeDasharray={l.status === "proposed" ? "5 4" : undefined} markerEnd={`url(#${id}-arrow)`} />;
      })}
      {kinds.map((k, i) => <g key={k.kind} className={landscapeStyles.form} style={tint(k.kind)}><LandscapeSolid x={i * 200 + 95} y={90} height={k.kind === d.kind ? 44 : 24} /></g>)}
    </svg>
    <div className={styles.contextKinds} style={{ gridTemplateColumns: `repeat(${Math.max(1, kinds.length)}, minmax(0, 1fr))` }}>
      {kinds.map(k => <div key={k.kind} style={tint(k.kind)} data-context-kind={k.kind}>
        <strong>{humanLabel(k.kind)}</strong><span>{k.entities.toLocaleString("en-AU")} objects · {STATUS[k.status].label}</span>
        {k.kind === d.kind && (d.type === "property" || d.type === "metric") && <span className={styles.contextPin}>{humanLabel(d.type)}: {humanLabel(d.name)}</span>}
      </div>)}
    </div>
    <ul className={styles.contextLinks}>
      {links.map((l, i) => <li key={i}>{humanLabel(l.from_kind)} → {humanLabel(l.label)} → {humanLabel(l.to_kind)} <span>{STATUS[l.status].label} · {l.count.toLocaleString("en-AU")} recorded links</span></li>)}
    </ul>
    <p className={styles.dim}>{total ? `${links.length} of ${total} direct link types shown.` : "No recorded links for this kind."} Arrows show direction. Dashed routes are suggestions.</p>
  </figure>;
}
