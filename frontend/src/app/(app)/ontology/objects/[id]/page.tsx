import { notFound } from "next/navigation";
import { Suspense } from "react";
import TwinArea from "@/components/twin/TwinArea";
export default async function Page({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  let decoded: string;
  try {
    decoded = decodeURIComponent(id);
  } catch {
    notFound();
  }
  return (
    <Suspense fallback={<p>Loading object…</p>}>
      <TwinArea view="object" kind={decoded} />
    </Suspense>
  );
}
