import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import { Suspense } from "react";
import TwinArea from "@/components/twin/TwinArea";

export default function Page() {
  return (
    <Suspense fallback={<LoadingSkeleton label="Loading the ontology" layout="table" />}>
      <TwinArea view="review" />
    </Suspense>
  );
}
