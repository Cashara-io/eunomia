import {
  memo,
  type CSSProperties,
  type Dispatch,
  type SetStateAction,
  type RefObject,
  type KeyboardEvent,
} from "react";
import Link from "next/link";
import {
  CubeIcon,
  UsersIcon,
  PackageIcon,
  ShoppingCartIcon,
  ReceiptIcon,
  MapPinIcon,
  WarehouseIcon,
  TruckIcon,
  ChatCircleDotsIcon,
  ClockCountdownIcon,
} from "@phosphor-icons/react";
import type { LinkItem } from "@/lib/queries/ontology";
import type { overviewLayout } from "./overview-layout";
import { linkKey } from "./overview-layout";
import { humanLabel, plural, linkSentence } from "./overview-format";
import type { LandscapePin, Highlight } from "./KindSummary";
import styles from "./overview.module.css";
const icons: Record<string, typeof CubeIcon> = {
  customer: UsersIcon,
  employee: UsersIcon,
  person: UsersIcon,
  product: PackageIcon,
  stock_item: PackageIcon,
  order: ShoppingCartIcon,
  purchase_order: ShoppingCartIcon,
  invoice: ReceiptIcon,
  payment: ReceiptIcon,
  location: MapPinIcon,
  warehouse: WarehouseIcon,
  shipment: TruckIcon,
  delivery: TruckIcon,
  delivery_route: TruckIcon,
};

type Props = {
  node: ReturnType<typeof overviewLayout>["nodes"][number];
  token: string;
  planeHeight: number;
  firstUnlinked: string | undefined;
  links: LinkItem[];
  pins: LandscapePin[];
  name: string;
  selected: boolean;
  tint: (kind: string) => CSSProperties;
  nodeRefs: RefObject<Map<string, HTMLButtonElement>>;
  choose: (item: Highlight) => void;
  keyNode: (event: KeyboardEvent<HTMLButtonElement>, kind: string) => void;
  setHovered: Dispatch<SetStateAction<Highlight | null>>;
  setFocused: Dispatch<SetStateAction<Highlight | null>>;
  setPinGroup: Dispatch<
    SetStateAction<{ kind: string; type: LandscapePin["type"] } | null>
  >;
};
export default memo(function LandscapeLandmark({
  node,
  token,
  planeHeight,
  firstUnlinked,
  links,
  pins,
  name,
  selected,
  tint,
  nodeRefs,
  choose,
  keyNode,
  setHovered,
  setFocused,
  setPinGroup,
}: Props) {
  const { kind, x, y, height, linked } = node;
  const Icon = icons[kind.kind] ?? CubeIcon;
  const item: Highlight = { type: "kind", key: kind.kind };
  return (
    <div
      key={kind.kind}
      className={styles.landmark}
      data-form={token}
      data-linked={linked}
      style={{
        ...tint(kind.kind),
        left: `${x / 12}%`,
        top: `${((y - height - 80) / planeHeight) * 100}%`,
      }}
    >
      {!linked && firstUnlinked === kind.kind && (
        <span className={styles.unlinkedLabel}>Not linked yet</span>
      )}
      <button
        ref={(el) => {
          if (el) nodeRefs.current.set(kind.kind, el);
          else nodeRefs.current.delete(kind.kind);
        }}
        data-kind={kind.kind}
        className={styles.landmarkButton}
        aria-label={name}
        aria-pressed={selected}
        onMouseEnter={() => setHovered(item)}
        onFocus={() => {
          setHovered(null);
          setFocused(item);
        }}
        onBlur={() => setFocused(null)}
        onClick={() => choose(item)}
        onKeyDown={(e) => keyNode(e, kind.kind)}
      >
        <span className={styles.kindTitle}>
          <Icon size={17} weight="duotone" aria-hidden />
          <span title={plural(humanLabel(kind.kind))}>
            {plural(humanLabel(kind.kind))}
          </span>
        </span>
        <span className={styles.nodeCount}>
          {kind.entities.toLocaleString("en-AU")}{" "}
          {plural("object", kind.entities)}
        </span>
      </button>
      {pins.some((p) => p.kind === kind.kind) && (
        <div className={styles.attentionPins}>
          {(["proposal", "stale"] as const).map((type) => {
            const group = pins.filter(
              (p) => p.kind === kind.kind && p.type === type,
            );
            const pin = group[0];
            if (!pin) return null;
            const count = group.reduce((sum, p) => sum + (p.count ?? 1), 0);
            const name = `${plural(humanLabel(kind.kind))}: ${count} ${type === "proposal" ? plural("proposal", count) + " to review" : plural("stale value", count)}. ${pin.label}`;
            const PinIcon =
              type === "proposal" ? ChatCircleDotsIcon : ClockCountdownIcon;
            return group.length > 1 ? (
              <button
                key={type}
                className={styles.attentionPin}
                data-pin-type={type}
                aria-label={name}
                title={name}
                onClick={() => setPinGroup({ kind: kind.kind, type })}
              >
                <PinIcon size={16} weight="bold" aria-hidden />
                <span>{count}</span>
              </button>
            ) : (
              <Link
                key={type}
                href={pin.href}
                className={styles.attentionPin}
                aria-label={name}
                title={name}
                data-pin-type={type}
              >
                <PinIcon size={16} weight="bold" aria-hidden />
                <span>{count}</span>
              </Link>
            );
          })}
        </div>
      )}
      <ul className={styles.flatLinks}>
        {links
          .filter((l) => l.from_kind === kind.kind)
          .map((l) => (
            <li key={linkKey(l)}>
              <button
                onClick={() => choose({ type: "link", key: linkKey(l) })}
                onFocus={() => {
                  setHovered(null);
                  setFocused({ type: "link", key: linkKey(l) });
                }}
                onBlur={() => setFocused(null)}
              >
                {linkSentence(l)}{" "}
                <span className={styles.nodeCount}>
                  {l.count.toLocaleString("en-AU")}
                </span>
              </button>
            </li>
          ))}
        {!linked && <li>No recorded links for this kind.</li>}
      </ul>
    </div>
  );
});
