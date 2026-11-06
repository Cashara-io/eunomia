import Link from "next/link";
import EunomiaMark from "@/components/EunomiaMark";
import { InputModeTracker } from "@/components/bits/motion";
import { AuthArt, AuthBand } from "@/components/Shapes";

/** Split layout shared by /login and /register: on the left a raised panel
 * with the product promise in plain words above a large Bauhaus composition
 * in the three vault colours (decorative, hidden from screen readers); the
 * form sits on white on the right. Below `lg` the panel collapses to the
 * wordmark over a thin band of the same shapes. */
export default function AuthShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="min-h-screen w-full flex flex-col lg:grid lg:grid-cols-[minmax(0,0.9fr)_minmax(0,1fr)]" style={{ background: "var(--surface)" }}>
      <aside className="relative flex flex-col overflow-hidden lg:min-h-screen" style={{ background: "var(--surface-raised)" }} aria-label="About Eunomia">
        <div className="relative z-10 flex flex-col px-6 pt-6 sm:px-10 lg:px-12 lg:pt-12">
          <Link href="/login" className="inline-flex items-center gap-2.5 self-start text-[20px] font-semibold tracking-[-0.03em] rounded-[4px]">
            <EunomiaMark size={28} />
            eunomia
          </Link>
          <div className="hidden lg:flex flex-col gap-8 pt-16">
            <p className="text-[clamp(40px,3.6vw,56px)] leading-[1.04] font-medium tracking-[-0.04em] max-w-[13ch]">
              Your context. Ready when you are.
            </p>
            <ol className="flex flex-col gap-2 text-[15px] leading-snug max-w-[40ch] list-decimal pl-5 marker:text-[var(--ink-faint)]" style={{ color: "var(--ink)" }}>
              <li>Choose the AI app you already use.</li>
              <li>Follow its steps.</li>
              <li>Sign in and approve access.</li>
              <li>Ask it to remember, and to recall.</li>
            </ol>
          </div>
        </div>
        <AuthArt className="hidden lg:block absolute bottom-0 right-0 w-full h-[46%]" />
        <AuthBand className="lg:hidden block w-full h-[72px] mt-4" />
      </aside>
      <div className="flex-1 flex flex-col px-6 py-6 sm:px-10">
        <main className="flex-1 flex items-start sm:items-center justify-center pt-8 pb-12 sm:py-12">
          <div className="w-full max-w-[420px]">{children}</div>
        </main>
        <p className="label text-center lg:text-left">Self-hosted. Free for personal use.</p>
      </div>
      <InputModeTracker />
    </div>
  );
}

export const LABEL = "text-[14px] font-medium";

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
