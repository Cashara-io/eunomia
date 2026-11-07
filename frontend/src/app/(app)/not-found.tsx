import Link from "next/link";
import EmptyState from "@/components/ui/EmptyState";
import PageHeader from "@/components/ui/PageHeader";

/** A missing page inside the app shell (raised with `notFound()` from a page in this group). */
export default function NotFound() {
  return (
    <div className="flex flex-col gap-2 max-w-[1240px]">
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
    </div>
  );
}
