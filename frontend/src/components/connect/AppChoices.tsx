"use client";

import Link from "next/link";
import { AI_APPS, AppMark, type AppId } from "./apps";

/** The "Which AI app do you use?" choices: neutral, nothing preselected, each
 * opening that app's own steps at /connect/[app]. Shared by Home and onboarding.
 * `onPick` runs first (onboarding finishes setup) and then navigates itself. */
export default function AppChoices({ onPick, disabled = false }: { onPick?: (id: AppId) => void; disabled?: boolean }) {
  return (
    // Columns follow the space the grid sits in, not the window: four across on Home, two in the
    // narrower onboarding panel, one on a phone. Each card is at least 220px, so names never cut and the panel never strands one card.
    <ul className="grid gap-3 grid-cols-[repeat(auto-fit,minmax(220px,1fr))]" aria-label="AI apps">
      {AI_APPS.map((a) => (
        <li key={a.id} className="min-w-0">
          <Link
            href={`/connect/${a.id}`}
            aria-disabled={disabled || undefined}
            onClick={(e) => {
              if (!onPick) return;
              e.preventDefault();
              if (!disabled) onPick(a.id);
            }}
            className="flex flex-col gap-1.5 h-full min-w-0 min-h-[76px] px-4 py-3.5 rounded-[3px] transition-colors hover:bg-[var(--bauhaus-app-bg)]"
            style={{ border: "1px solid var(--bauhaus-app-ink)" }}
          >
            {/* the name stays whole on one line; the short hint may wrap */}
            <span className="flex items-center gap-2.5 text-[16px] font-semibold whitespace-nowrap">
              <AppMark id={a.id} size={22} />
              {a.name}
            </span>
            <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              {a.hint}
            </span>
          </Link>
        </li>
      ))}
    </ul>
  );
}
