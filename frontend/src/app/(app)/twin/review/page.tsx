import { Suspense } from "react";
import TwinArea from "@/components/twin/TwinArea";

export default function Page() {
  return (
    <Suspense fallback={<p>Loading Business twin…</p>}>
      <TwinArea view="review" />
    </Suspense>
  );
}
