import { Suspense } from "react";
import TwinArea from "@/components/twin/TwinArea";
export default async function Page({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  return <Suspense fallback={<p>Loading object…</p>}><TwinArea view="object" kind={id} /></Suspense>;
}
