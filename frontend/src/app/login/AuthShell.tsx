import Link from "next/link";
import EunomiaMark from "@/components/EunomiaMark";
import { InputModeTracker } from "@/components/bits/motion";

/** Split layout shared by /login and /register (spec App 01, 01.2): a soft
 * panel on the left with the product promise in plain words, the form on the
 * right. No decorative imagery beyond three flat Bauhaus shapes, which are
 * hidden from screen readers. Below `lg` the panel collapses to the wordmark. */
export default function AuthShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen w-full grid lg:grid-cols-[minmax(0,0.8fr)_minmax(0,1fr)]" style={{ background: "var(--bauhaus-app-bg)" }}>
      <aside
        className="flex flex-col px-6 py-6 sm:px-10 lg:px-12 lg:py-12"
        style={{ background: "var(--bauhaus-app-soft)" }}
        aria-label="About Eunomia"
      >
        <Link href="/login" className="inline-flex items-center gap-2 self-start text-[20px] font-semibold tracking-[-0.03em] rounded-[3px]">
          <EunomiaMark size={24} />
          eunomia
        </Link>
        <div className="hidden lg:flex flex-1 flex-col justify-center gap-10 py-12">
          <p className="text-[clamp(40px,4vw,56px)] leading-[1.05] font-semibold tracking-[-0.04em]">
            Your context.
            <br />
            Ready when you are.
          </p>
          <ol className="flex flex-col gap-2.5 text-[15px] leading-snug max-w-[40ch]" style={{ color: "var(--ink)" }}>
            <li>1. Choose the AI app you already use.</li>
            <li>2. Follow its steps.</li>
            <li>3. Sign in and approve access.</li>
            <li>4. Ask it to remember, and to recall.</li>
          </ol>
          <div className="flex items-center gap-3" aria-hidden>
            <span className="w-14 h-14 rounded-full" style={{ background: "var(--bauhaus-app-red)" }} />
            <span className="w-14 h-14" style={{ background: "var(--bauhaus-app-yellow)" }} />
            <span className="w-28 h-14 rounded-full" style={{ background: "var(--bauhaus-app-blue)" }} />
          </div>
        </div>
        <p className="hidden lg:block caps" style={{ color: "var(--ink-dim)" }}>
          Shared memory for the AI apps you already use
        </p>
      </aside>
      <div className="flex flex-col px-6 py-6 sm:px-10">
        <main className="flex-1 flex items-start sm:items-center justify-center pt-10 pb-12 sm:py-12">
          <div className="w-full max-w-[420px]">{children}</div>
        </main>
        <p className="label text-center lg:text-left">Self-hosted. Free for personal use.</p>
      </div>
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
