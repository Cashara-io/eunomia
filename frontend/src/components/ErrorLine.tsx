import { WarningCircleIcon } from "@phosphor-icons/react/ssr";
import { ApiError } from "@/lib/api";
import CopyButton from "@/components/bits/CopyButton";

export type Failure = { message: string; code?: string; traceId?: string };

/* Turns a caught value into a Failure, keeping the server message when present. */
export function failure(e: unknown, fallback: string, suffix = ""): Failure {
  const base = e instanceof Error && e.message ? e.message : fallback;
  // a server message may lack its full stop: "... 'x'" + " Check the address" must still read as two sentences
  const message = suffix && !/[.!?]$/.test(base) ? `${base}.${suffix}` : base + suffix;
  return e instanceof ApiError ? { message, code: e.code, traceId: e.traceId } : { message };
}

/** An inline error: names the problem in words, offers at most one recovery
 * action beside it, and keeps the technical details (code, trace id) quiet
 * and copyable for whoever helps. Never shows credentials or stack traces. */
export default function ErrorLine({
  error,
  children,
  action,
}: {
  error?: Failure | null;
  children?: React.ReactNode;
  /** One useful next step, e.g. a "Try again" button or a link to the right setting. */
  action?: React.ReactNode;
}) {
  const showCode = error?.code && !error.code.startsWith("http.");
  return (
    <div
      role="alert"
      className="flex flex-wrap items-start gap-x-3 gap-y-2 px-3.5 py-2.5 rounded-[10px]"
      style={{ background: "var(--critical-soft)" }}
    >
      <WarningCircleIcon weight="duotone" size={18} aria-hidden className="shrink-0 mt-px" style={{ color: "var(--critical)" }} />
      <div className="flex-1 min-w-[200px] flex flex-col gap-1">
        <p className="text-[14px] leading-snug" style={{ color: "var(--critical)" }}>
          {children ?? error?.message}
        </p>
        {(showCode || error?.traceId) && (
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {showCode && <code className="font-mono">{error!.code}</code>}
            {error?.traceId && (
              <span className="flex items-center gap-1">
                <span>Trace</span>
                <code className="font-mono">{error.traceId}</code>
                <CopyButton value={error.traceId} label="Copy trace id" />
              </span>
            )}
          </p>
        )}
      </div>
      {action && <div className="shrink-0">{action}</div>}
    </div>
  );
}
