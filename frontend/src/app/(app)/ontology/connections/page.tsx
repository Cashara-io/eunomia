import { Suspense } from "react";
import TwinArea from "@/components/twin/TwinArea";
export default function Page() {
  return <Suspense fallback={<p>Loading connections…</p>}><TwinArea view="connections" /></Suspense>;
}
