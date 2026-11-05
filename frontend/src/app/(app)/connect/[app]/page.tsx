"use client";

import { use, useState } from "react";
import Link from "next/link";
import CopyButton from "@/components/bits/CopyButton";
import ConnectAgents from "@/components/ConnectAgents";
import { appById, connectionFor, type AiApp, type Connection } from "@/components/connect/apps";
import { useConnections } from "@/components/connect/useConnections";
import {
  Approved,
  CheckConnection,
  CommandBlock,
  EXAMPLE_QUESTION,
  GuideFrame,
  GuideStep,
  Kbd,
  Note,
  OsPicker,
  isPrivateHost,
  useMcpAddress,
} from "@/components/connect/Guide";

// Commands verified against docs/agents.md ("Sign in with OAuth", Claude Code, Codex, ChatGPT).
const claudeCommand = (url: string) => `claude mcp add --transport http --scope user eunomia ${url}`;
const codexCommand = (url: string) => `codex mcp add eunomia --url ${url}\ncodex mcp login eunomia`;

function LocalNote({ url, cloud }: { url: string; cloud: boolean }) {
  if (!url) return null;
  const host = new URL(url).hostname;
  const https = url.startsWith("https://");
  if (cloud && (!https || isPrivateHost(host)))
    return (
      <div role="note" className="soft-panel px-5 py-4 text-[14px] leading-relaxed mb-2">
        <strong>ChatGPT cannot reach this address yet.</strong> ChatGPT connects from OpenAI&apos;s servers, so Eunomia needs a public{" "}
        <Kbd>https://</Kbd> address, not <Kbd>{url.replace(/\/mcp$/, "")}</Kbd>. Set one up under{" "}
        <Link href="/settings?tab=https" className="underline" style={{ color: "var(--accent-text)" }}>
          Settings, HTTPS
        </Link>{" "}
        (or ask the person who installed Eunomia), then open this page from that address.
      </div>
    );
  if (!cloud && isPrivateHost(host))
    return (
      <Note>
        This address only works for apps on this {host === "localhost" || host.startsWith("127.") ? "computer" : "network"}. To connect an app on another
        computer, give Eunomia a public address under{" "}
        <Link href="/settings?tab=https" className="underline" style={{ color: "var(--accent-text)" }}>
          Settings, HTTPS
        </Link>
        .
      </Note>
    );
  return null;
}

/** For people who prefer a pasted key over signing in, or whose app can't sign in. */
function TokenFallback({ initial }: { initial: "claude" | "codex" | "other" }) {
  return (
    <li className="guide-step py-6">
    <details>
      <summary className="cursor-pointer text-[15px] font-semibold min-h-11 flex items-center">Advanced: connect with an API token instead</summary>
      <div className="pt-3 flex flex-col gap-3">
        <Note>Use this only if your app cannot open the Eunomia sign-in page. The token is shown once; you can revoke it any time under Settings, API tokens.</Note>
        <ConnectAgents initialClient={initial} />
      </div>
    </details>
    </li>
  );
}

function ChatGptGuide({ app, url, onConfirmed }: { app: AiApp; url: string; onConfirmed: (c: Connection) => void }) {
  return (
    <GuideFrame id="chatgpt" intro="Use ChatGPT on the web for setup. Keep this page open beside it. Your Eunomia address is ready to copy below.">
      <LocalNote url={url} cloud />
      <ol className="flex flex-col" style={{ borderTop: "1px solid var(--border)" }}>
        <GuideStep n={1} title="Open Plugins in ChatGPT">
          <p>Open the link below and sign in to ChatGPT. Choose the plus (+) button, then Add custom MCP server.</p>
          <a href="https://chatgpt.com/plugins" target="_blank" rel="noreferrer" className="btn btn-primary self-start">
            Open ChatGPT Plugins
          </a>
        </GuideStep>
        <GuideStep n={2} title="Add Eunomia with these details">
          <dl className="soft-panel grid grid-cols-[minmax(0,140px)_minmax(0,1fr)] gap-x-4 gap-y-2.5 px-5 py-4 text-[14px]">
            <dt style={{ color: "var(--ink-dim)" }}>Name</dt>
            <dd>Eunomia</dd>
            <dt style={{ color: "var(--ink-dim)" }}>Server URL</dt>
            <dd className="font-mono [overflow-wrap:anywhere]">{url}</dd>
            <dt style={{ color: "var(--ink-dim)" }}>Authentication</dt>
            <dd>OAuth (sign in with Eunomia)</dd>
          </dl>
          <CopyButton value={url} label="Copy server URL" size="sm" className="self-start" />
          <Note>Review the warning, select I understand and want to continue, then choose Create as a plugin. There is nothing else to paste.</Note>
        </GuideStep>
        <GuideStep n={3} title="Approve access, then install">
          <p>
            When the Eunomia sign-in page opens, use the same email and password you use here. Review what ChatGPT is asking for and choose Allow. Back in
            ChatGPT, find Eunomia in your personal plugins and choose Install.
          </p>
        </GuideStep>
        <GuideStep n={4} title="Use Eunomia in a new chat">
          <p>
            Start a new ChatGPT conversation. Type <Kbd>@</Kbd> and choose Eunomia before asking your question, for example: &ldquo;{EXAMPLE_QUESTION}&rdquo;
          </p>
          <CheckConnection app={app} onConfirmed={onConfirmed} />
          <aside className="soft-panel px-5 py-4 flex flex-col gap-2 mt-2" aria-labelledby="no-custom">
            <h4 id="no-custom" className="text-[15px] font-semibold">
              Can&apos;t see &ldquo;Add custom MCP server&rdquo;?
            </h4>
            <Note>
              Your workspace may not allow custom connections. Business, Enterprise and Edu plans can use them fully; Pro can read memories but not save new
              ones. Ask your workspace administrator to enable them, or choose a different AI app.
            </Note>
            <Link href="/connect/help#chatgpt" className="btn btn-sm self-start">
              View ChatGPT setup help
            </Link>
          </aside>
        </GuideStep>
      </ol>
    </GuideFrame>
  );
}

function TerminalGuide({ app, url, onConfirmed }: { app: AiApp; url: string; onConfirmed: (c: Connection) => void }) {
  const claude = app.id === "claude-code";
  return (
    <GuideFrame id={app.id} intro={`These instructions are for ${app.name} on your computer. Keep this page open while you follow them.`}>
      <ol className="flex flex-col" style={{ borderTop: "1px solid var(--border)" }}>
        <GuideStep n={1} title="Open your computer’s command window">
          <OsPicker />
        </GuideStep>
        <GuideStep n={2} title="Copy and run this command">
          <Note>
            Copy the command, paste it into that window, and press Enter.
            {claude ? " It adds Eunomia to Claude Code for all your projects." : " The second line starts sign-in."}
          </Note>
          <CommandBlock value={claude ? claudeCommand(url) : codexCommand(url)} />
          <LocalNote url={url} cloud={false} />
        </GuideStep>
        <GuideStep n={3} title="Sign in and approve access">
          {claude ? (
            <>
              <p>
                Open Claude Code and type <Kbd>/mcp</Kbd>. Select eunomia and follow the sign-in prompt to open your browser. Use your Eunomia email and
                password, review the permissions, and choose Allow.
              </p>
              <Note>If eunomia is not listed, make sure the command in step 2 finished without an error. Connection help shows what to do if it did not.</Note>
            </>
          ) : (
            <>
              <p>
                Codex asks you to sign in. Follow its prompt to open the browser. Use your Eunomia email and password, review the permissions, and choose
                Allow.
              </p>
              <Note>
                No sign-in prompt? Run <Kbd>codex mcp login eunomia</Kbd> again, or open Connection help for the exact next step.
              </Note>
            </>
          )}
        </GuideStep>
        <GuideStep n={4} title={`Start a new ${app.name} session`}>
          <p>
            Close your current {app.name} session and open a new one. Then ask: &ldquo;{EXAMPLE_QUESTION}&rdquo;
          </p>
          <CheckConnection app={app} onConfirmed={onConfirmed} />
        </GuideStep>
        <TokenFallback initial={claude ? "claude" : "codex"} />
      </ol>
    </GuideFrame>
  );
}

function OtherGuide({ app, url, onConfirmed }: { app: AiApp; url: string; onConfirmed: (c: Connection) => void }) {
  const details = `Name: Eunomia\nServer URL: ${url}\nAuthentication: OAuth (sign in with Eunomia)`;
  return (
    <GuideFrame
      id="other"
      intro="Some AI apps can connect to Eunomia from their settings. If your app does not show a connection option, ask its support team or the person who installed Eunomia."
    >
      <ol className="flex flex-col" style={{ borderTop: "1px solid var(--border)" }}>
        <GuideStep n={1} title="Find the connection option in your app">
          <p>Look for Add connector, Add MCP server, or custom connection in the app&apos;s settings. The wording depends on the app.</p>
        </GuideStep>
        <GuideStep n={2} title="Enter Eunomia’s address and sign in">
          <p>
            Use <Kbd>{url}</Kbd>. If asked for Authentication, choose OAuth. Use your Eunomia email and password when the sign-in page opens.
          </p>
          <LocalNote url={url} cloud={false} />
        </GuideStep>
        <GuideStep n={3} title="Approve and start a new conversation">
          <p>Review the requested access and choose Allow. Follow your app&apos;s instructions to turn on Eunomia in a new conversation.</p>
          <CopyButton value={details} label="Copy connection details" size="sm" className="btn-primary self-start" />
          <CheckConnection app={app} onConfirmed={onConfirmed} />
        </GuideStep>
        <TokenFallback initial="other" />
      </ol>
    </GuideFrame>
  );
}

export default function ConnectAppPage({ params }: { params: Promise<{ app: string }> }) {
  const { app: id } = use(params);
  const app = appById(id);
  const url = useMcpAddress();
  const { connections } = useConnections();
  // Set by "Check connection" when the server confirms; null = show the steps.
  const [confirmed, setConfirmed] = useState<Connection | null>(null);
  const [showSteps, setShowSteps] = useState(false);

  if (!app)
    return (
      <div className="flex flex-col gap-4 max-w-[720px]">
        <h1 className="page-title">We don&apos;t have steps for that app</h1>
        <p className="page-intro">Choose your AI app on Home to see its steps.</p>
        <Link href="/" className="btn btn-primary self-start">
          Choose your AI app
        </Link>
      </div>
    );

  // Already connected (confirmed by the server) when the page opened: say so first.
  const existing = connections ? connectionFor(app, connections) : undefined;
  const shown = confirmed ?? (showSteps ? null : existing ?? null);
  if (shown) return <Approved connection={shown} onShowSteps={() => {
          setConfirmed(null);
          setShowSteps(true);
        }} />;

  if (app.id === "chatgpt") return <ChatGptGuide app={app} url={url} onConfirmed={setConfirmed} />;
  if (app.id === "other") return <OtherGuide app={app} url={url} onConfirmed={setConfirmed} />;
  return <TerminalGuide app={app} url={url} onConfirmed={setConfirmed} />;
}
