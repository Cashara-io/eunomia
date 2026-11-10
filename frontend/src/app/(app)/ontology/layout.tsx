import { Suspense, type ReactNode } from "react";
import OntologyAccess from "@/components/twin/OntologyAccess";

export default function OntologyLayout({ children }: { children: ReactNode }) {
  return <Suspense fallback={null}><OntologyAccess>{children}</OntologyAccess></Suspense>;
}
