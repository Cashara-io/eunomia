"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Link from "next/link";
import { useState } from "react";
import ConfirmButton from "@/components/ConfirmButton";
import Markdown from "@/components/Markdown";
import CopyButton from "@/components/bits/CopyButton";
import SyncMark from "@/components/bits/SyncMark";
import Tooltip from "@/components/bits/Tooltip";
import { useSettings, useUpdateSettings } from "@/lib/queries/settings";

const FRONTMATTER = /^---\n([\s\S]*?)\n---\n/;
const REFRESH = "./scripts/connect-agents.sh --token <token>";

// Where the skill goes once saved. Mirrors docs/agents.md "The memory skill".
const ROUTES = [
  { who: "Every MCP client", how: "Sent as the server's connection instructions and offered as the eunomia-memory prompt. Applies the next time the agent connects." },
  { who: "Claude Code, Codex, Hermes", how: "Written to skills/eunomia-memory/SKILL.md. Re-run connect-agents.sh to refresh the file." },
  { who: "Claude Code hooks (optional)", how: "SessionStart injects this skill. UserPromptSubmit recalls memories for each prompt." },
];

// The skill that tells connected agents when to recall and what to remember.
// Sent to every MCP client as connection instructions and an MCP prompt, injected
// into Claude Code each session by its optional hook, and written as SKILL.md by connect-agents.sh.
export default function SkillPage() {
  const settingsQuery = useSettings();
  const updateSettings = useUpdateSettings();
  const skill = settingsQuery.data?.memory_skill ?? null;
  const custom = settingsQuery.data?.memory_skill_custom ?? false;
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setError] = useState<Failure | null>(null);
  const loadError = settingsQuery.isError ? failure(settingsQuery.error, "Could not load the skill.", " Check that the backend is running, then reload.") : null;
  const error = saveError ?? loadError;

  async function save(text: string) {
    setBusy(true);
    setError(null);
    try {
      await updateSettings.mutateAsync({ memory_skill: text });
      setEditing(false);
      setSaved(true);
      setTimeout(() => setSaved(false), 1800);
    } catch (e) {
      setError(failure(e, "Could not save the skill."));
    } finally {
      setBusy(false);
    }
  }

  const meta = Object.fromEntries(
    (skill?.match(FRONTMATTER)?.[1] ?? "")
      .split("\n")
      .map((l) => l.match(/^(\w+):\s*(.*)$/))
      .filter((m): m is RegExpMatchArray => !!m)
      .map((m) => [m[1], m[2]]),
  );
  const dirty = editing && draft !== skill;

  return (
    <div className="max-w-[1120px] flex flex-col gap-8">
      <header className="flex flex-col gap-2 max-w-[68ch]">
        <h1 className="page-title">How your agents use memory</h1>
        <p className="text-[13.5px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
          This skill tells every connected agent when to recall and what to remember. Edit it here and new agent sessions
          pick it up.
        </p>
      </header>

      <div className="grid gap-8 grid-cols-[minmax(0,1fr)] lg:grid-cols-[minmax(0,1fr)_300px] items-start">
        <section className="ledger min-w-0 overflow-hidden" aria-label="Skill" style={editing ? { borderColor: "var(--accent)" } : undefined}>
          <div className="flex items-center gap-2 flex-wrap px-4 py-2.5" style={{ borderBottom: "var(--hair) solid var(--border)" }}>
            <span className="font-mono text-[12.5px]" style={{ color: "var(--ink)" }}>
              SKILL.md
            </span>
            {skill === null ? (
              <span className="skeleton h-5 w-24" />
            ) : (
              <span className="pill" aria-label="Skill source">
                {custom ? "Customised" : "Built-in default"}
              </span>
            )}
            <div className="flex items-center gap-2 ml-auto">
            {skill !== null &&
              (editing ? (
                <>
                  <button onClick={() => setEditing(false)} disabled={busy} className="btn btn-sm btn-ghost">
                    Cancel
                  </button>
                  <button onClick={() => save(draft)} disabled={busy || !draft.trim() || !dirty} className="btn btn-sm btn-primary">
                    {busy ? "Saving…" : "Save"}
                  </button>
                </>
              ) : (
                <>
                  {custom && (
                    <ConfirmButton label="Reset to default" confirmLabel="Replace with built-in" onConfirm={() => save("")} disabled={busy} className="btn btn-sm btn-ghost">
                      Reset to default
                    </ConfirmButton>
                  )}
                  <button
                    onClick={() => {
                      setDraft(skill);
                      setEditing(true);
                    }}
                    className="btn btn-sm"
                  >
                    Edit
                  </button>
                  <CopyButton value={skill} label="Copy SKILL.md" size="sm" className="btn-primary" />
                </>
              ))}
            </div>
          </div>

          {/* Always mounted (hidden when idle) so the check draws each time Saved appears. */}
          <div className={saved || error ? "px-4 pt-3" : "hidden"}>
            <p className={`${saved ? "flex" : "hidden"} items-center gap-1.5 text-[12.5px] fade-in`} style={{ color: "var(--good)" }} role="status">
              <SyncMark status={saved ? "done" : "idle"} />
              Saved. New agent sessions pick it up.
            </p>
            {error && <ErrorLine error={error} />}
          </div>

          {skill === null ? (
            !error && (
              <div className="flex flex-col gap-3 p-6 md:p-10" aria-busy="true" aria-label="Loading skill">
                <span className="skeleton h-7 w-56 mb-3" />
                {[100, 92, 96, 70, 0, 40, 98, 90, 64].map((w, i) =>
                  w ? <span key={i} className="skeleton h-3.5" style={{ width: `${w}%` }} /> : <span key={i} className="h-3" />,
                )}
              </div>
            )
          ) : editing ? (
            <textarea
              aria-label="Skill markdown"
              autoFocus
              className="block w-full min-h-[64vh] resize-y bg-transparent px-5 py-5 md:px-8 font-mono text-[12.5px] leading-relaxed focus:outline-none"
              style={{ color: "var(--ink)" }}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if ((e.metaKey || e.ctrlKey) && e.key === "Enter" && draft.trim() && dirty) save(draft);
                if (e.key === "Escape") setEditing(false);
              }}
              spellCheck={false}
            />
          ) : (
            <>
              {(meta.name || meta.description) && (
                <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 md:px-10 pt-6 text-[13px]">
                  {meta.name && (
                    <>
                      <dt className="label pt-px">Name</dt>
                      <dd className="font-mono text-[12.5px]">{meta.name}</dd>
                    </>
                  )}
                  {meta.description && (
                    <>
                      <dt className="label pt-px">Triggers on</dt>
                      <dd className="leading-relaxed max-w-[64ch]" style={{ color: "var(--ink-dim)" }}>
                        {meta.description}
                      </dd>
                    </>
                  )}
                </dl>
              )}
              <article className="docs px-5 pt-6 pb-8 md:px-10 md:pb-10 text-[14px] leading-[1.7]">
                <div className="mb-6" style={{ borderTop: "var(--hair) solid var(--border)" }} />
                <Markdown text={skill.replace(FRONTMATTER, "")} />
              </article>
            </>
          )}
          {editing && (
            <p className="label px-5 md:px-8 py-2.5 flex gap-3" style={{ borderTop: "var(--hair) solid var(--border)" }}>
              <span>
                <span className="kbd">⌘</span> <span className="kbd">Enter</span> to save
              </span>
              <span>
                <span className="kbd">Esc</span> to cancel
              </span>
            </p>
          )}
        </section>

        <aside className="flex flex-col gap-8 lg:sticky lg:top-8">
          <section className="flex flex-col gap-3">
            <h2 className="section-title">Where it goes</h2>
            <ul className="ledger hairline-rows">
              {ROUTES.map((r) => (
                <li key={r.who} className="px-4 py-3 flex flex-col gap-1">
                  <span className="text-[13px] font-medium">{r.who}</span>
                  <span className="text-[12.5px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
                    {r.how}
                  </span>
                </li>
              ))}
            </ul>
          </section>

          <section className="flex flex-col gap-3">
            <h2 className="section-title">Refresh file copies</h2>
            <p className="text-[12.5px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
              Run from the Eunomia repo with a token from{" "}
              <Link href="/settings" style={{ color: "var(--accent-text)" }} className="underline">
                Settings
              </Link>
              .
            </p>
            <div className="ledger flex items-center gap-2 pl-3 pr-1 h-10">
              <code className="font-mono text-[12px] flex-1 min-w-0 truncate" title={REFRESH}>
                {REFRESH}
              </code>
              <Tooltip label="Copy command">
                <CopyButton value={REFRESH} label="Copy command" />
              </Tooltip>
            </div>
          </section>
        </aside>
      </div>
    </div>
  );
}
