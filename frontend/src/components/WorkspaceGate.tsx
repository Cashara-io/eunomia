"use client";

import { useSyncExternalStore } from "react";
import { WarningCircleIcon } from "@phosphor-icons/react/ssr";
import { useRouter } from "next/navigation";
import { DataSection } from "@/app/(app)/settings/DataSection";
import { PasswordSection } from "@/app/(app)/settings/PasswordSection";
import { useLogout, useMe } from "@/lib/queries/auth";
import { useOrgPlan } from "@/lib/queries/org";

/** Between the sign-in check and the app: a workspace its operator has suspended (a hosted service, when payment
 * fails) shows one clear page instead of an app whose every request is refused. Everyone else, and any server that
 * sets no plans, goes straight through. While the plan is loading the app waits: opening it only to have its
 * requests refused is the wall of errors this page replaces. If the plan can't be read the app opens as usual. */
export default function WorkspaceGate({ children }: { children: React.ReactNode }) {
  const plan = useOrgPlan();
  if (plan.isPending) return null;
  if (plan.data?.status !== "suspended") return <>{children}</>;
  return <SuspendedPage manageUrl={plan.data.manage_url ?? null} />;
}

/** The account screens a suspended workspace's member can still use: the server lets exactly these routes through
 * (`plan::allowed_while_suspended`), so the page shows them itself rather than sending the person to Settings, which
 * is part of the workspace and is refused. Chosen by the address's `#fragment`, so each is a real link. */
export type AccountView = "password" | "data";

export function accountViewOf(hash: string): AccountView | null {
  const v = hash.replace(/^#/, "");
  return v === "password" || v === "data" ? v : null;
}

const subscribeToHash = (onChange: () => void) => {
  window.addEventListener("hashchange", onChange);
  return () => window.removeEventListener("hashchange", onChange);
};

function SuspendedPage({ manageUrl }: { manageUrl: string | null }) {
  const hash = useSyncExternalStore(
    subscribeToHash,
    () => window.location.hash,
    () => ""
  );
  return <Suspended manageUrl={manageUrl} view={accountViewOf(hash)} />;
}

export function Suspended({ manageUrl, view }: { manageUrl: string | null; view: AccountView | null }) {
  const me = useMe().data;
  const router = useRouter();
  const logout = useLogout();
  const isOwner = me?.org_role === "owner";
  return (
    <main id="main" className="flex-1 flex items-center justify-center px-4 py-10" data-testid="workspace-suspended">
      <div className={`flex flex-col gap-4 w-full ${view ? "max-w-2xl" : "max-w-lg"}`}>
        <WarningCircleIcon weight="duotone" size={28} aria-hidden style={{ color: "var(--critical)" }} />
        <h1 className="text-[22px] font-medium">This workspace is paused</h1>
        <p className="text-[15px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
          Its subscription needs attention, so nobody can open its memory or use its connections for now. Nothing has been deleted: everything is
          kept exactly as it was, and it comes back as soon as the workspace is active again.
        </p>
        {isOwner ? (
          <p className="text-[15px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
            You&apos;re the owner, so this is yours to sort out.
            {manageUrl ? " Open the billing page to see what is needed." : " Contact whoever runs this service to restore it."}
          </p>
        ) : (
          <p className="text-[15px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
            Ask the workspace owner{me?.can_manage_org ? "" : " or an admin"} to restore it.
          </p>
        )}
        <div className="flex flex-wrap gap-2">
          {isOwner && manageUrl && (
            <a href={manageUrl} className="btn btn-primary min-h-11" data-testid="workspace-manage">
              Manage the subscription
            </a>
          )}
          <button
            type="button"
            className="btn min-h-11"
            onClick={async () => {
              await logout.mutateAsync().catch(() => {});
              router.replace("/login");
            }}
          >
            Sign out
          </button>
        </div>
        <nav aria-label="Your account" className="flex flex-col gap-1.5 text-[14px]" data-testid="workspace-account-links">
          <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            You can still change your password, and export or delete your own data.
          </p>
          <div className="flex flex-wrap gap-x-5 gap-y-1">
            <a href="#password" className="underline" aria-current={view === "password" ? "page" : undefined} data-testid="workspace-link-password">
              Change your password
            </a>
            <a href="#data" className="underline" aria-current={view === "data" ? "page" : undefined} data-testid="workspace-link-data">
              Export or delete your data
            </a>
          </div>
        </nav>
        {view === "password" && (
          <section aria-label="Change your password" data-testid="workspace-view-password">
            <PasswordSection />
          </section>
        )}
        {view === "data" && (
          <section aria-label="Your data" data-testid="workspace-view-data">
            <DataSection paused />
          </section>
        )}
      </div>
    </main>
  );
}
