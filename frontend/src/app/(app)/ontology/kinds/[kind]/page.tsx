import { Suspense } from "react";
import TwinArea from "@/components/twin/TwinArea";

export default async function Page({
  params,
}: {
  params: Promise<{ kind: string }>;
}) {
  const { kind } = await params;
  return (
    <Suspense fallback={<p>Loading kind…</p>}>
      <TwinArea view="kind" kind={kind} />
    </Suspense>
  );
}
