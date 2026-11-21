"use client";

import { WarningCircleIcon } from "@phosphor-icons/react/ssr";
import { useRouter } from "next/navigation";
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
  return <Suspended manageUrl={plan.data.manage_url ?? null} />;
}

function Suspended({ manageUrl }: { manageUrl: string | null }) {
  const me = useMe().data;
  const router = useRouter();
  const logout = useLogout();
  const isOwner = me?.org_role === "owner";
  return (
    <main id="main" className="flex-1 flex items-center justify-center px-4 py-10" data-testid="workspace-suspended">
      <div className="flex flex-col gap-4 max-w-lg">
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
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          You can still sign in, change your password, and export or delete your own data from your account.
        </p>
      </div>
    </main>
  );
}
