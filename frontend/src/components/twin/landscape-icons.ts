import { CubeIcon, UsersIcon, PackageIcon, ShoppingCartIcon, ReceiptIcon, MapPinIcon, WarehouseIcon, TruckIcon } from "@phosphor-icons/react";
export const landscapeIcons: Record<string, typeof CubeIcon> = {
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

