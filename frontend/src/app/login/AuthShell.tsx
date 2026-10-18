import Link from "next/link";
import EunomiaMark from "@/components/EunomiaMark";
import ThemeToggle from "@/components/ThemeToggle";
import { InputModeTracker } from "@/components/bits/motion";
import AgentCanvas from "./AgentCanvas";

/** Split layout shared by /login and /register: a compact form on the left,
 * and on wide screens an illustrative canvas where several agents converge
 * on one shared memory. The canvas is a client island beside a server-rendered
 * form, so it never delays the form. It is a demonstration of the mechanism,
 * not data: the agent names are clients Eunomia connects to over MCP. */
export default function AuthShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen w-full grid lg:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)]">
      <div className="flex flex-col px-6 py-6 sm:px-10">
        <header className="flex items-center justify-between">
          <Link href="/login" className="inline-flex items-center gap-2 text-[14px] font-semibold tracking-[-0.01em]">
            <EunomiaMark size={22} />
            Eunomia
          </Link>
          <ThemeToggle />
        </header>
        <main className="flex-1 flex items-start sm:items-center justify-center pt-16 pb-12 sm:py-12">
          <div className="w-full max-w-[340px]">{children}</div>
        </main>
        <p className="label">Self-hosted. Free for personal use.</p>
      </div>
      <AgentCanvas />
      <InputModeTracker />
    </div>
  );
}

export const LABEL = "text-[12.5px] font-medium";

/** Show/hide for a password field. Named "Show"/"Hide" (not "...password")
 * so it never collides with the field's own accessible name. */
export function RevealToggle({ shown, onToggle, controls }: { shown: boolean; onToggle: () => void; controls: string }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-pressed={shown}
      aria-controls={controls}
      className="btn btn-ghost btn-sm absolute right-[5px] top-[5px]"
    >
      {shown ? "Hide" : "Show"}
    </button>
  );
}
