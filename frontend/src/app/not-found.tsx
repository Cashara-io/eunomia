import Link from "next/link";
import EmptyState from "@/components/ui/EmptyState";
import EunomiaMark from "@/components/EunomiaMark";

/** Unmatched URLs, outside the app shell (no sidebar: the visitor may not be signed in). */
export default function NotFound() {
  return (
    <div className="min-h-screen w-full flex flex-col px-6 py-6 sm:px-10" style={{ background: "var(--bauhaus-app-bg)" }}>
      <Link href="/" className="inline-flex items-center gap-2 text-[20px] font-semibold tracking-[-0.03em] self-start">
        <EunomiaMark size={24} />
        eunomia
      </Link>
      <main className="flex-1 flex flex-col justify-center max-w-[680px] w-full mx-auto">
        <h1 className="text-[20px] font-semibold">Page not found</h1>
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
