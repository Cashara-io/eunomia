"use client";

import Link from "next/link";
import { AI_APPS, AppMark, type AppId } from "./apps";

/** The "Which AI app do you use?" choices: neutral, nothing preselected, each
 * opening that app's own steps at /connect/[app]. Shared by Home and onboarding.
 * `onPick` runs first (onboarding finishes setup) and then navigates itself. */
export default function AppChoices({ onPick, disabled = false }: { onPick?: (id: AppId) => void; disabled?: boolean }) {
  return (
    <ul className="grid gap-3 grid-cols-1 sm:grid-cols-2 xl:grid-cols-4" aria-label="AI apps">
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
            {/* one line each: the name never wraps, the hint is cut short rather than wrapping */}
            <span className="flex items-center gap-2.5 text-[16px] font-semibold whitespace-nowrap min-w-0">
              <AppMark id={a.id} size={22} />
              <span className="truncate">{a.name}</span>
            </span>
            <span className="text-[13px] truncate" style={{ color: "var(--ink-dim)" }} title={a.hint}>
              {a.hint}
            </span>
          </Link>
        </li>
      ))}
    </ul>
  );
}
