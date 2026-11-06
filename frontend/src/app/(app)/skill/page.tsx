"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Link from "next/link";
import { useState } from "react";
import ConfirmButton from "@/components/ConfirmButton";
import Markdown from "@/components/Markdown";
import CopyButton from "@/components/bits/CopyButton";
import SyncMark from "@/components/bits/SyncMark";
import PageHeader from "@/components/ui/PageHeader";
import { BrainCircuit } from "lucide-react";
import Tooltip from "@/components/bits/Tooltip";
import { useSettings, useUpdateSettings } from "@/lib/queries/settings";

const FRONTMATTER = /^---\n([\s\S]*?)\n---\n/;
const REFRESH = "./scripts/connect-agents.sh --token <token>";

// Where the skill goes once saved. Mirrors docs/agents.md "The memory skill".
const ROUTES = [
  { who: "Every connected AI app", how: "Sent as the connection instructions (and the eunomia-memory prompt). Applies the next time the app connects." },
  { who: "Claude Code, Codex, Hermes", how: "Also saved as skills/eunomia-memory/SKILL.md on the computer. Re-run connect-agents.sh to refresh that file." },
  { who: "Claude Code hooks (optional)", how: "SessionStart adds these instructions to each session. UserPromptSubmit looks up memories for each prompt." },
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
  const loadError = settingsQuery.isError ? failure(settingsQuery.error, "Could not load the AI instructions.", " Check that the backend is running, then reload.") : null;
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
      setError(failure(e, "Could not save the AI instructions."));
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
    <div className="max-w-[1040px] flex flex-col gap-7">
      <PageHeader
        icon={BrainCircuit}
        title="AI instructions"
        intro="How your AI apps use memory: these instructions tell every connected AI app when to look things up in Eunomia and what to save. Edit them here; new sessions pick up your changes."
      />

      <section className="min-w-0 flex flex-col gap-3" aria-label="Instructions">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="font-mono text-[13px]" style={{ color: "var(--accent-text)" }}>
            SKILL.md
          </span>
          {skill === null ? (
            <span className="skeleton h-5 w-24" />
          ) : (
            <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
              ·{" "}
              <span aria-label="Skill source">{custom ? "Customised" : "Built-in default"}</span>
            </span>
          )}
          <div className="flex flex-wrap items-center gap-2 ml-auto">
            {skill !== null &&
              (editing ? (
                <>
                  <button onClick={() => setEditing(false)} disabled={busy} className="btn">
                    Cancel
                  </button>
                  <button onClick={() => save(draft)} disabled={busy || !draft.trim() || !dirty} className="btn btn-primary">
                    {busy ? "Saving…" : "Save instructions"}
                  </button>
                </>
              ) : (
                <>
                  {custom && (
                    <ConfirmButton label="Reset to default" confirmLabel="Replace with built-in" onConfirm={() => save("")} disabled={busy} className="btn btn-ghost">
                      Reset to default
                    </ConfirmButton>
                  )}
                  <button
                    onClick={() => {
                      setDraft(skill);
                      setEditing(true);
                    }}
                    className="btn"
                  >
                    Edit
                  </button>
                  <CopyButton value={skill} label="Copy SKILL.md" size="sm" className="btn-primary" />
                </>
              ))}
          </div>
        </div>

        {/* Always mounted (hidden when idle) so the check draws each time Saved appears. */}
        <div className={saved || error ? "" : "hidden"}>
          <p className={`${saved ? "flex" : "hidden"} items-center gap-1.5 text-[14px] fade-in`} style={{ color: "var(--ink)" }} role="status">
            <SyncMark status={saved ? "done" : "idle"} />
            Saved. New AI app sessions pick up the change.
          </p>
          {error && <ErrorLine error={error} />}
        </div>

        <div className="soft-panel min-w-0 overflow-hidden" style={editing ? { boxShadow: "inset 0 0 0 1px var(--ink)" } : undefined}>
          {skill === null ? (
            !error && (
              <div className="flex flex-col gap-3 p-6 md:p-10" aria-busy="true" aria-label="Loading instructions">
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
              className="block w-full min-h-[64vh] resize-y bg-transparent px-5 py-5 md:px-8 font-mono text-[13px] leading-relaxed focus:outline-none"
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
                <dl className="grid grid-cols-[96px_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 md:px-8 pt-6 text-[14px]">
                  {meta.name && (
                    <>
                      <dt className="label pt-px">Name</dt>
                      <dd className="font-mono text-[13px]">{meta.name}</dd>
                    </>
                  )}
                  {meta.description && (
                    <>
                      <dt className="label pt-px">Used when</dt>
                      <dd className="leading-relaxed max-w-[64ch]" style={{ color: "var(--ink-dim)" }}>
                        {meta.description}
                      </dd>
                    </>
                  )}
                </dl>
              )}
              <article className="docs px-5 pt-6 pb-8 md:px-8 md:pb-10 text-[15px] leading-[1.7]">
                <div className="mb-6" style={{ borderTop: "1px solid var(--border)" }} />
                <Markdown text={skill.replace(FRONTMATTER, "")} />
              </article>
            </>
          )}
          {editing && (
            <p className="label px-5 md:px-8 py-2.5 flex flex-wrap gap-3" style={{ borderTop: "1px solid var(--border)" }}>
              <span>Markdown</span>
              <span>
                <span className="kbd">⌘</span> <span className="kbd">Enter</span> to save
              </span>
              <span>
                <span className="kbd">Esc</span> to cancel
              </span>
            </p>
          )}
        </div>
      </section>

      <section className="flex flex-col gap-3" aria-labelledby="where-title">
        <h2 id="where-title" className="section-title">
          Where it goes
        </h2>
        <div className="ledger overflow-x-auto">
          <table className="data-table">
            <thead>
              <tr>
                <th>Destination</th>
                <th>How it applies</th>
              </tr>
            </thead>
            <tbody>
              {ROUTES.map((r) => (
                <tr key={r.who}>
                  <td className="font-medium align-top py-3 whitespace-nowrap">{r.who}</td>
                  <td className="py-3 leading-relaxed" style={{ color: "var(--ink-dim)" }}>
                    {r.how}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          These are the shared instructions for every app. They are not the same as reusable skills an AI app saves for
          itself.
        </p>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-[14px] font-medium">Refresh file copies</h2>
        <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
          Only needed for the file copies. Run this from the Eunomia install folder with a token from{" "}
          <Link href="/settings?tab=tokens" style={{ color: "var(--accent-text)" }} className="underline">
            Settings, API tokens
          </Link>
          .
        </p>
        <div className="field flex items-center gap-2 pl-3 pr-1 min-h-11">
          <code className="font-mono text-[13px] flex-1 min-w-0 truncate" title={REFRESH}>
            {REFRESH}
          </code>
          <Tooltip label="Copy command">
            <CopyButton value={REFRESH} label="Copy command" />
          </Tooltip>
        </div>
      </section>
    </div>
  );
}
