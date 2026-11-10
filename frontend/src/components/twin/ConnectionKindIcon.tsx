import {
  CubeIcon,
  ShoppingCartIcon,
  MapPinIcon,
  TruckIcon,
  ReceiptIcon,
  UsersIcon,
  ShapesIcon,
} from "@phosphor-icons/react";
export default function ConnectionKindIcon({ kind }: { kind: string }) {
  const Icon =
    {
      product: CubeIcon,
      order: ShoppingCartIcon,
      location: MapPinIcon,
      shipment: TruckIcon,
      invoice: ReceiptIcon,
      supplier: UsersIcon,
    }[kind] ?? ShapesIcon;
  return <Icon size={16} weight="duotone" aria-hidden />;
}
