"use client";

import { CheckCircleIcon } from "@phosphor-icons/react/ssr";
import { useRef, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import CopyButton from "@/components/bits/CopyButton";
import PageHeader from "@/components/ui/PageHeader";
import { AppMark, appById, appForConnection, connectionFor, type AiApp, type AppId, type Connection } from "./apps";
import { useConnections } from "./useConnections";

export const EXAMPLE_QUESTION = "Use Eunomia to look for memories about me.";

/** "Connect your AI app / ChatGPT": the way back to Home, then where you are. */
export function Trail({ here }: { here: string }) {
  return (
    <nav aria-label="Breadcrumb" className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
      <Link href="/" className="underline underline-offset-2" style={{ color: "var(--accent-text)" }}>
        Connect your AI app
      </Link>{" "}
      / <span aria-current="page">{here}</span>
    </nav>
  );
}

export function GuideStep({ n, title, children }: { n?: number; title: React.ReactNode; children: React.ReactNode }) {
  return (
    <li className="guide-step flex flex-col gap-3 py-6">
      <h3 className="text-[17px] font-medium leading-snug tracking-[-0.015em]">
        {n !== undefined && <span>{n}. </span>}
        {title}
      </h3>
      <div className="flex flex-col gap-3 text-[15px] leading-relaxed" style={{ color: "var(--ink)" }}>
        {children}
      </div>
    </li>
  );
}

export const Note = ({ children }: { children: React.ReactNode }) => (
  <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
    {children}
  </p>
);

export const Kbd = ({ children }: { children: React.ReactNode }) => <code className="font-mono text-[0.92em] px-1 rounded-[4px]" style={{ background: "var(--surface-raised)" }}>{children}</code>;

/** A command to paste into a terminal: a raised code block, then one primary copy button. */
export function CommandBlock({ value, label = "Command" }: { value: string; label?: string }) {
  return (
    <div className="flex flex-col gap-3 items-start">
      <pre
        aria-label={label}
        className="w-full font-mono text-[13px] leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] px-4 py-3.5 rounded-[10px]"
        style={{ background: "var(--surface-raised)", color: "var(--ink)", border: "1px solid var(--border)" }}
      >
        {value}
      </pre>
      <CopyButton value={value} label={`Copy ${label.toLowerCase()}`} size="sm" className="btn-primary" />
    </div>
  );
}

type Os = "mac" | "windows" | "linux";
const OS: { id: Os; label: string; how: React.ReactNode }[] = [
  { id: "mac", label: "Mac", how: <>On your Mac, press <Kbd>⌘ Space</Kbd>, type Terminal, then press Enter.</> },
  { id: "windows", label: "Windows", how: <>Press the Windows key, type PowerShell, then press Enter.</> },
  { id: "linux", label: "Linux", how: <>Open your Terminal app. On many systems the shortcut is <Kbd>Ctrl Alt T</Kbd>.</> },
];

function guessOs(): Os {
  const p = navigator.userAgent;
  return /Windows/i.test(p) ? "windows" : /Linux|X11/i.test(p) && !/Android/i.test(p) ? "linux" : "mac";
}
const noSubscribe = () => () => {};

/** Step 1 of a terminal guide: which computer, and how to open its command window. */
export function OsPicker() {
  const detected = useSyncExternalStore(noSubscribe, guessOs, () => "mac" as Os);
  const [chosen, setChosen] = useState<Os | null>(null);
  const os = chosen ?? detected;
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  return (
    <>
      <div role="radiogroup" aria-label="Your computer" className="flex flex-wrap gap-2">
        {OS.map((o, i) => (
          <button
            key={o.id}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={os === o.id}
            tabIndex={os === o.id ? 0 : -1}
            onClick={() => setChosen(o.id)}
            onKeyDown={(e) => {
              const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
              if (!step) return;
              e.preventDefault();
              const j = (i + step + OS.length) % OS.length;
              setChosen(OS[j].id);
              refs.current[j]?.focus();
            }}
            className="pill min-h-10 px-4"
          >
            {o.label}
          </button>
        ))}
      </div>
      <Note>{OS.find((o) => o.id === os)!.how}</Note>
    </>
  );
}

/** Same origin as the app: the frontend proxies /mcp to the backend. */
export function useMcpAddress(): string {
  return useSyncExternalStore(noSubscribe, () => `${window.location.origin}/mcp`, () => "");
}

/** Only reachable from this computer or this network. */
export function isPrivateHost(host: string): boolean {
  return (
    host === "localhost" ||
    host.endsWith(".localhost") ||
    host.endsWith(".local") ||
    host === "[::1]" ||
    /^127\./.test(host) ||
    /^10\./.test(host) ||
    /^192\.168\./.test(host) ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(host) ||
    /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(host)
  );
}

type CheckState = { status: "idle" | "checking" | "not-yet" | "failed" };

/** Step 4's buttons: ask Eunomia whether the app has really signed in. Success
 * only comes from the server (an approved sign-in, or a token the app used). */
export function CheckConnection({ app, onConfirmed }: { app: AiApp; onConfirmed: (c: Connection) => void }) {
  const { recheck } = useConnections();
  const [state, setState] = useState<CheckState>({ status: "idle" });
  async function check() {
    setState({ status: "checking" });
    try {
      const found = connectionFor(app, await recheck());
      if (found) onConfirmed(found);
      else setState({ status: "not-yet" });
    } catch {
      setState({ status: "failed" });
    }
  }
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-2">
        <button type="button" className="btn btn-primary" onClick={check} disabled={state.status === "checking"} aria-busy={state.status === "checking"}>
          {state.status === "checking" ? "Checking…" : "Check connection"}
        </button>
        <Link href="/connect/help" className="btn">
          Connection help
        </Link>
      </div>
      <div aria-live="polite">
        {state.status === "not-yet" && (
          <p className="soft-panel px-4 py-3 text-[14px] leading-relaxed">
            <strong>Not connected yet.</strong> Eunomia has not seen a sign-in from {app.name}. Finish step 3 (sign in and choose Allow), then check again.
          </p>
        )}
        {state.status === "failed" && (
          <p className="text-[14px]" style={{ color: "var(--critical)" }}>
            Could not reach Eunomia to check. Reload the page, then try again.
          </p>
        )}
      </div>
      <Note>A successful search can find no memories. That means the connection works; there is simply nothing saved yet.</Note>
    </div>
  );
}

/** App 02.4: shown only once the server has confirmed the connection. */
export function Approved({ connection, onShowSteps }: { connection: Connection; onShowSteps?: () => void }) {
  const app = appForConnection(connection);
  const where =
    app.id === "chatgpt"
      ? "In ChatGPT, start a new chat, type @ and choose Eunomia, then paste the question."
      : app.id === "other"
        ? `In ${connection.name}, start a new conversation and paste the question.`
        : `Start a new ${app.name} session and paste the question.`;
  return (
    <div className="flex flex-col gap-8 max-w-[960px]">
      <PageHeader icon={CheckCircleIcon} tone="project" title="Connection approved" intro={<Trail here="Ready to use" />} />
      <section className="task-panel p-6 sm:p-8 flex flex-col gap-5" aria-labelledby="approved-title">
        <h2 id="approved-title" className="connect-title">
          Your AI app is connected
        </h2>
        <p className="text-[16px]">
          Eunomia has confirmed {connection.via === "sign-in" ? "your sign-in" : "that this app is using its key"}. Your app can now use the memories you approved.
        </p>
        <div className="soft-panel px-5 py-4 flex flex-col gap-1.5">
          <p className="text-[16px] font-semibold" style={{ color: "var(--accent-text)" }}>
            {connection.name} · {connection.via === "sign-in" ? "Access approved" : "Connected with an API token"}
          </p>
          <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
            Your connection is ready. If no memories have been saved yet, a search can return no results. That is normal.
          </p>
        </div>
        <div className="flex flex-col gap-3 items-start px-5 py-4" style={{ border: "1px solid var(--border-strong)", borderRadius: 12 }}>
          <p className="text-[16px]">{EXAMPLE_QUESTION}</p>
          <CopyButton value={EXAMPLE_QUESTION} label="Copy example question" size="sm" className="btn-primary" />
        </div>
        <Note>
          {where} To save your first memory, ask your AI app to remember something (if it has permission to save), or add one from People &amp; topics in Eunomia.
        </Note>
        <div className="flex flex-wrap gap-2">
          {app.openUrl ? (
            <a href={app.openUrl} target="_blank" rel="noreferrer" className="btn btn-primary">
              Open {app.name}
            </a>
          ) : (
            <Link href="/" className="btn btn-primary">
              Back to Home
            </Link>
          )}
          <Link href="/settings?tab=apps" className="btn">
            View connected apps
          </Link>
          {onShowSteps && (
            <button type="button" className="btn btn-ghost" onClick={onShowSteps}>
              Show the setup steps again
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

/** Frame for one app's guide: header with the app's mark, then the framed steps. */
export function GuideFrame({ id, intro, children }: { id: AppId; intro: React.ReactNode; children: React.ReactNode }) {
  const app = appById(id)!;
  const title = id === "other" ? "Other AI apps" : `Connect ${app.name}`;
  return (
    <div className="flex flex-col gap-8 max-w-[960px]">
      <PageHeader mark={<AppMark id={id} size={30} />} title={title} intro={<Trail here={id === "other" ? "More options" : app.name} />} />
      <section className="task-panel p-6 sm:p-8 flex flex-col" aria-labelledby="guide-title">
        <div className="flex flex-col gap-4 pb-6">
          <h2 id="guide-title" className="connect-title">
            {id === "other" ? "Connect another AI app" : `Connect ${app.name}`}
          </h2>
          <p className="text-[16px] leading-relaxed">{intro}</p>
          <Link href="/" className="btn self-start">
            Choose a different app
          </Link>
        </div>
        {children}
      </section>
    </div>
  );
}
