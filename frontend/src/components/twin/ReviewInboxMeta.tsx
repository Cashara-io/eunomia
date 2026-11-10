import { CubeIcon, LinkIcon, SlidersHorizontalIcon, FunctionIcon } from "@phosphor-icons/react/ssr";
import type { Definition } from "./model";
import { humanLabel, ReviewTime } from "./review-format";
import { proposalEvent, proposerFallback } from "./review-activity";
import { landscapeIcons } from "./landscape-icons";
import styles from "./review.module.css";

export function ReviewTypeIcon({ definition: d }: { definition: Definition }) {
  const Icon = d.type === "kind" ? landscapeIcons[d.kind] ?? CubeIcon
    : d.type === "link" ? LinkIcon : d.type === "metric" ? FunctionIcon : SlidersHorizontalIcon;
  return <Icon size={18} weight="duotone" aria-hidden />;
}

export default function ReviewInboxMeta({ proposer, definition: d }: { proposer: ReturnType<typeof proposalEvent>; definition: Definition }) {
  return <>
    <span className={styles.inboxChange}>
      {d.type === "link" ? `${humanLabel(d.item.from_kind)} → ${humanLabel(d.item.label)} → ${humanLabel(d.item.to_kind)}`
        : d.type === "property" ? `Allow ${humanLabel(d.item.datatype)} values on ${humanLabel(d.kind)}`
        : d.type === "metric" ? d.item.expr_words
        : `Allow ${humanLabel(d.kind)} in strict mode`}
    </span>
    <span className={styles.attribution}>
      {proposer ? proposer.actor_name : proposerFallback(d)}
      {proposer ? <> · <ReviewTime at={proposer.at} /></> : " · Time not recorded"}
    </span>
  </>;
}
