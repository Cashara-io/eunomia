import { CubeIcon, PackageIcon, UsersIcon, ShoppingCartIcon, ReceiptIcon, MapPinIcon, WarehouseIcon, TruckIcon } from "@phosphor-icons/react";
import { useKindColor } from "@/lib/kindColors";
import type { ObjectProfile } from "@/lib/queries/objects";
import { humanLabel } from "./overview-format";
import { formatted } from "./ObjectProfile";
import styles from "./receipts.module.css";
const icons: Record<string, typeof CubeIcon> = { product: PackageIcon, stock_item: PackageIcon, customer: UsersIcon, employee: UsersIcon, person: UsersIcon, order: ShoppingCartIcon, purchase_order: ShoppingCartIcon, invoice: ReceiptIcon, payment: ReceiptIcon, location: MapPinIcon, warehouse: WarehouseIcon, shipment: TruckIcon, delivery: TruckIcon };
const families: Record<string, string> = { product: "repository", stock_item: "repository", customer: "person", employee: "person", supplier: "person", order: "company", purchase_order: "company", invoice: "file", payment: "file", delivery: "organisation", shipment: "organisation", warehouse: "location" };
export default function ObjectHero({ data, compact }: { data: ObjectProfile; compact: boolean }) {
  const color = useKindColor();
  const Icon = icons[data.kind] ?? CubeIcon;
  const keys = data.properties.filter(p => ["money", "number", "integer", "percent"].includes(p.datatype) && p.value !== null && p.state !== "unavailable").slice(0, 2);
  return <header className={styles.hero} style={{ borderTopColor: color(families[data.kind] ?? data.kind) }}>
    <div className={styles.identity}>
      <span className={styles.kindIcon} style={{ background: color(families[data.kind] ?? data.kind) }}><Icon size={compact ? 28 : 38} weight="duotone" aria-hidden /></span>
      <div><p className={styles.plate}>{humanLabel(data.kind)} · Object receipt</p><h2>{data.name}</h2><p>{data.summary || "No summary recorded."}</p></div>
    </div>
    {!compact && <dl className={styles.keyNumbers}>{keys.map(p => <div key={p.name}><dt>{humanLabel(p.name)}</dt><dd>{formatted(p.value, p)}</dd><dd className={styles.numberNote}>{p.state === "stale" ? "Uses a stale source" : p.state === "conflicting" ? "Conflicting values" : "Current recorded value"}</dd></div>)}</dl>}
  </header>;
}
