import Link from "next/link";
import EunomiaMark from "@/components/EunomiaMark";
import { InputModeTracker } from "@/components/bits/motion";

/** Layout shared by /login and /register: the wordmark, a centred form, and
 * the "by Paperless" mark underneath. Nothing decorative. */
export default function AuthShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen w-full flex flex-col items-center px-4 py-10 sm:py-16" style={{ background: "var(--canvas)" }}>
      <main className="w-full max-w-[420px] flex-1 flex flex-col gap-10 sm:justify-center">
        <Link href="/login" className="inline-flex items-center gap-2 self-center text-[20px] font-semibold tracking-[-0.03em] rounded-[3px]">
          <EunomiaMark size={24} />
          eunomia
        </Link>
        <div className="w-full">{children}</div>
      </main>
      <a
        href="https://www.buildpaperless.com.au/eunomia"
        target="_blank"
        rel="noreferrer"
        className="mt-12 flex items-center gap-1.5 text-[12px] rounded-[3px]"
        style={{ color: "var(--ink-dim)" }}
        aria-label="Made by Paperless (opens in a new tab)"
      >
        <span aria-hidden>by</span>
        {/* eslint-disable-next-line @next/next/no-img-element -- a fixed 560x192 PNG, nothing to optimise */}
        <img src="/paperless-logo-dark.png" alt="" width={70} height={24} className="block h-[18px] w-auto" />
      </a>
      <InputModeTracker />
    </div>
  );
}

export const LABEL = "text-[14px] font-semibold";

/** Show/hide for a password field. Named "Show"/"Hide" (not "...password")
 * so it never collides with the field's own accessible name. */
export function RevealToggle({ shown, onToggle, controls }: { shown: boolean; onToggle: () => void; controls: string }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-pressed={shown}
      aria-controls={controls}
      className="btn btn-ghost btn-sm absolute right-[4px] top-[4px] min-h-9!"
    >
      {shown ? "Hide" : "Show"}
    </button>
  );
}
