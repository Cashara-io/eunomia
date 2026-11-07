"use client";

import { LifebuoyIcon } from "@phosphor-icons/react/ssr";
import Link from "next/link";
import CopyButton from "@/components/bits/CopyButton";
import PageHeader from "@/components/ui/PageHeader";
import { useMe } from "@/lib/queries/auth";
import { EXAMPLE_QUESTION, Kbd, Note, useMcpAddress } from "@/components/connect/Guide";

function Problem({ id, title, children }: { id?: string; title: string; children: React.ReactNode }) {
  return (
    <section id={id} className="guide-step flex flex-col gap-3 py-6 scroll-mt-6" aria-labelledby={id ? `${id}-title` : undefined}>
      <h3 id={id ? `${id}-title` : undefined} className="text-[17px] font-medium tracking-[-0.015em]">
        {title}
      </h3>
      {children}
    </section>
  );
}

/** App 02.6: help for the common ways connecting goes wrong. Plain words, one next step each. */
export default function ConnectionHelpPage() {
  const url = useMcpAddress();
  const isAdmin = useMe().data?.is_admin ?? false;
  return (
    <div className="flex flex-col gap-8 max-w-[960px]">
      <PageHeader icon={LifebuoyIcon} tone="personal" title="Connection help" intro="Help with connecting your AI app." />
      <div className="task-panel p-6 sm:p-8 flex flex-col">
        <div className="flex flex-col gap-3 pb-6" style={{ borderBottom: "1px solid var(--border)" }}>
          <h2 className="connect-title">Let&apos;s get your app connected</h2>
          <p className="text-[16px]">Choose the issue that matches what you see. You do not need to change technical settings yourself.</p>
        </div>

        <Problem id="choose" title="I’m not sure which app to choose">
          <Note>
            ChatGPT is the website or desktop app. Claude Code is a coding tool in a terminal, not the Claude chat website. Codex is OpenAI&apos;s coding app
            and terminal tool. Pick the one you already use.
          </Note>
          <Link href="/" className="btn self-start">
            Choose my app
          </Link>
        </Problem>

        <Problem id="chatgpt" title="ChatGPT has no “Add custom MCP server” option">
          <Note>
            Your workspace may not allow it. Custom connections work on ChatGPT Business, Enterprise and Edu; on Pro, ChatGPT can read memories but not
            save new ones. Ask your workspace administrator to enable custom connections, or use another AI app.
          </Note>
          <Note>
            ChatGPT also needs Eunomia to have a public https:// address. If yours starts with http://localhost or a home network address, an admin can
            turn on HTTPS in Eunomia&apos;s settings.
          </Note>
          <div className="flex flex-wrap gap-2">
            <Link href="/connect/chatgpt" className="btn">
              View ChatGPT steps
            </Link>
            {isAdmin && (
              <Link href="/settings?tab=https" className="btn">
                Open HTTPS settings
              </Link>
            )}
          </div>
        </Problem>

        <Problem id="not-found" title="My command says “not found”">
          <Note>
            The coding app is not installed, or your command window cannot find it. Install the app first, then open a new command window and run the
            command again. You can also{" "}
            <Link href="/" className="link">
              choose another app
            </Link>{" "}
            instead of changing computer settings.
          </Note>
        </Problem>

        <Problem id="no-sign-in" title="No sign-in page opened">
          <Note>
            In Claude Code, type <Kbd>/mcp</Kbd>, select eunomia and follow the sign-in prompt. For Codex, run <Kbd>codex mcp login eunomia</Kbd>. In
            ChatGPT, make sure Authentication is set to OAuth.
          </Note>
          <div className="flex flex-wrap gap-2">
            <Link href="/connect/claude-code" className="btn">
              Claude Code steps
            </Link>
            <Link href="/connect/codex" className="btn">
              Codex steps
            </Link>
            <Link href="/connect/chatgpt" className="btn">
              ChatGPT steps
            </Link>
          </div>
        </Problem>

        <Problem id="no-memories" title="The app connected but found no memories">
          <Note>That is a normal result for a new account. Try asking the app to save a simple preference, then ask about it again.</Note>
          <p className="text-[15px]">&ldquo;{EXAMPLE_QUESTION}&rdquo;</p>
          <CopyButton value={EXAMPLE_QUESTION} label="Copy example question" size="sm" className="self-start" />
        </Problem>

        <section className="soft-panel px-5 py-5 mt-2 flex flex-col gap-3" aria-labelledby="unfinished-title">
          <h3 id="unfinished-title" className="text-[17px] font-medium tracking-[-0.015em]">
            Sign-in did not finish?
          </h3>
          <Note>
            Return to your app&apos;s connection steps and sign in again. If the Eunomia page will not open, copy its address and ask the person who installed
            Eunomia to check that it can be reached.
          </Note>
          <div className="flex flex-wrap gap-2">
            <Link href="/" className="btn btn-primary">
              Try sign-in again
            </Link>
            <CopyButton value={url} label="Copy Eunomia address" size="sm" />
          </div>
        </section>
      </div>
    </div>
  );
}
