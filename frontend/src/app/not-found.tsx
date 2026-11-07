import Link from "next/link";
import EmptyState from "@/components/ui/EmptyState";
import EunomiaMark from "@/components/EunomiaMark";
import PageHeader from "@/components/ui/PageHeader";

/** Unmatched URLs, outside the app shell (no sidebar: the visitor may not be signed in). Laid out like
 * the in-app one, `(app)/not-found.tsx`: the same header and message, top-left under the logo. */
export default function NotFound() {
  return (
    <div className="min-h-screen w-full flex flex-col gap-10 px-4 py-6 sm:px-8 lg:px-12" style={{ background: "var(--canvas)" }}>
      <Link href="/" className="inline-flex items-center gap-2 text-[20px] font-semibold tracking-[-0.03em] self-start">
        <EunomiaMark size={24} />
        eunomia
      </Link>
      <main className="flex flex-col gap-2 max-w-[1240px] w-full">
        <PageHeader title="Page not found" />
        <EmptyState
          action={
            <div className="flex flex-wrap gap-2">
              <Link href="/" className="btn btn-primary">
                Home
              </Link>
              <Link href="/docs" className="btn">
                Help
              </Link>
            </div>
          }
        >
          This page doesn&apos;t exist.
        </EmptyState>
      </main>
    </div>
  );
}
