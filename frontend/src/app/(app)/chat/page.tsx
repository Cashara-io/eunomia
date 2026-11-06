"use client";

import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import Select from "@/components/Select";
import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import { AlertCircle, ArrowUp, Square, ChevronRight, CornerDownLeft, MessagesSquare, RotateCcw, Plus, Trash2, Wrench } from "lucide-react";
import PageHeader from "@/components/ui/PageHeader";
import EmptyNote from "@/components/ui/EmptyState";
import Markdown from "@/components/Markdown";
import AuthorTag from "@/components/AuthorTag";
import SyncMark from "@/components/bits/SyncMark";
import Tooltip, { TooltipGroup } from "@/components/bits/Tooltip";
import { useQueryClient } from "@tanstack/react-query";
import { ApiError, chat } from "@/lib/api";
import type { ChatMessage, ChatThread } from "@/lib/types";
import { useMe } from "@/lib/queries/auth";
import { useSettings } from "@/lib/queries/settings";
import { chatKeys, historyQuery, threadsQuery, useCreateThread, useDeleteThread, useThreads } from "@/lib/queries/chat";

// The built-in agent, labelled by the name its system prompt gives it.
const ASSISTANT = "Eunomia";

const EXAMPLES = [
  "What do you remember about me?",
  "Which people and topics have you saved so far?",
  "Remember that I prefer Bun over npm for frontend work.",
  "Search my synced records for anything from last week.",
];

type ToolStep = { name: string; args: string; result?: string; running?: boolean };

// A rendered turn: one person's message, or everything the assistant did
// between two of them (tool calls, their results, and the reply text).
type Turn =
  | { kind: "user"; content: string; at?: string }
  | { kind: "assistant"; content: string; steps: ToolStep[]; at?: string };

function toTurns(messages: ChatMessage[]): Turn[] {
  const turns: Turn[] = [];
  const pending: ToolStep[] = [];
  for (const m of messages) {
    if (m.role === "user") {
      turns.push({ kind: "user", content: m.content, at: m.created_at });
      continue;
    }
    let last = turns[turns.length - 1];
    if (!last || last.kind !== "assistant") {
      last = { kind: "assistant", content: "", steps: [], at: m.created_at };
      turns.push(last);
    }
    if (m.role === "tool") {
      // tool results arrive in the same order as the calls that asked for them
      const step = pending.shift();
      if (step) step.result = m.content;
      else last.steps.push({ name: "tool", args: "", result: m.content });
      continue;
    }
    for (const tc of m.tool_calls ?? []) {
      const step = { name: tc.function.name, args: tc.function.arguments };
      last.steps.push(step);
      pending.push(step);
    }
    if (m.content) last.content = last.content ? `${last.content}\n\n${m.content}` : m.content;
  }
  return turns;
}

function timeLabel(at?: string | null): string {
  if (!at) return "";
  const d = new Date(at);
  if (Number.isNaN(d.getTime())) return "";
  return new Date().toDateString() === d.toDateString()
    ? d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : d.toLocaleDateString([], { month: "short", day: "numeric" });
}

// Compact age for the thread list: "now", "12m", "3h", "4d", then a date.
function ageLabel(at?: string | null): string {
  if (!at) return "";
  const d = new Date(at);
  const mins = Math.floor((Date.now() - d.getTime()) / 60000);
  if (Number.isNaN(mins)) return "";
  if (mins < 1) return "now";
  if (mins < 60) return `${mins}m`;
  if (mins < 1440) return `${Math.floor(mins / 60)}h`;
  if (mins < 10080) return `${Math.floor(mins / 1440)}d`;
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function prettyJson(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

function argsPreview(args: string): string {
  try {
    const parsed = JSON.parse(args);
    if (parsed && typeof parsed === "object") {
      return Object.entries(parsed)
        .map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`)
        .join("  ");
    }
  } catch {
    // not JSON, show as-is
  }
  return args;
}

const secs = (ms: number) => `${(Math.max(0, ms) / 1000).toFixed(1)}s`;

// Ticks on its own so the rest of the page does not re-render every 100ms.
// aria-hidden: a ticking number inside the live region would chatter.
function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(since);
  useEffect(() => {
    const t = setInterval(() => setNow(performance.now()), 100);
    return () => clearInterval(t);
  }, []);
  return (
    <span aria-hidden className="font-mono tabular-nums">
      {secs(now - since)}
    </span>
  );
}

// Fades the transcript's top or bottom edge only when there is more to scroll that way.
function edgeFade(el: HTMLElement) {
  el.toggleAttribute("data-fade-top", el.scrollTop > 1);
  el.toggleAttribute("data-fade-bottom", el.scrollHeight - el.scrollTop - el.clientHeight > 1);
}

function TurnHeader({ name, at, children }: { name: string; at?: string; children?: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2 h-5 mb-2">
      <AuthorTag name={name} />
      {at && (
        <time dateTime={at} className="font-mono text-[11.5px]" style={{ color: "var(--ink-faint)" }}>
          {timeLabel(at)}
        </time>
      )}
      {children}
    </div>
  );
}

function ToolRow({ step, live }: { step: ToolStep; live?: boolean }) {
  const preview = argsPreview(step.args);
  const inspectable = !step.running && !!(step.args || step.result);
  const row = (
    <>
      {inspectable ? (
        <ChevronRight size={13} strokeWidth={1.75} className="chev shrink-0" style={{ color: "var(--ink-faint)" }} />
      ) : (
        <Wrench size={13} strokeWidth={1.75} className="shrink-0" style={{ color: "var(--ink-faint)" }} />
      )}
      <span className="font-mono text-[12px] shrink-0" style={{ color: "var(--accent-text)" }}>
        {step.name}
      </span>
      {preview && (
        <span className="font-mono text-[12px] truncate min-w-0" style={{ color: "var(--ink-dim)" }}>
          {preview}
        </span>
      )}
      <span className="ml-auto pl-2 shrink-0 flex items-center gap-1 text-[12px]" style={{ color: "var(--ink-dim)" }}>
        {/* history rows mount "done" and sit still; live rows draw the check when their tool returns */}
        <SyncMark status={step.running ? "running" : "done"} size={12} />
        {step.running ? "Running" : "Done"}
      </span>
    </>
  );
  if (!inspectable) return <div className={`flex items-center gap-2 min-h-10 px-3 ${live ? "tool-live" : ""}`}>{row}</div>;
  return (
    <details className="tool-row">
      <summary className="press flex items-center gap-2 min-h-10 px-3 cursor-pointer list-none select-none [&::-webkit-details-marker]:hidden">
        {row}
      </summary>
      <div className="px-2.5 pt-1 pb-2.5 flex flex-col gap-2">
        {step.args && (
          <div>
            <div className="label mb-1">Arguments</div>
            <pre className="tool-pre">{prettyJson(step.args)}</pre>
          </div>
        )}
        {step.result && (
          <div>
            <div className="label mb-1">Result</div>
            <pre className="tool-pre">{prettyJson(step.result)}</pre>
          </div>
        )}
      </div>
    </details>
  );
}

function ToolSteps({ steps, live }: { steps: ToolStep[]; live?: boolean }) {
  if (steps.length === 0) return null;
  return (
    <div className="ledger hairline-rows mb-3 overflow-hidden" role="group" aria-label="Where this answer looked (tool calls)">
      {steps.map((s, i) => (
        <ToolRow key={i} step={s} live={live} />
      ))}
    </div>
  );
}

function TurnView({ turn, me }: { turn: Turn; me: string | null }) {
  if (turn.kind === "user") {
    return (
      <article>
        {me ? <TurnHeader name={me} at={turn.at} /> : <div className="h-5 mb-2" />}
        <div
          className="whitespace-pre-wrap break-words text-[15px] leading-relaxed px-4 py-3 rounded-[2px]"
          style={{ background: "var(--bauhaus-app-soft)", color: "var(--ink)" }}
        >
          {turn.content}
        </div>
      </article>
    );
  }
  return (
    <article>
      <TurnHeader name={ASSISTANT} at={turn.at} />
      <ToolSteps steps={turn.steps} />
      {turn.content && (
        <div className="text-[15px] px-0.5 break-words" style={{ color: "var(--ink)" }}>
          <Markdown text={turn.content} />
        </div>
      )}
    </article>
  );
}

function ThreadList({
  threads,
  activeId,
  onSelect,
  onCreate,
  onDelete,
}: {
  threads: ChatThread[] | null;
  activeId: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onDelete: (id: string) => void;
}) {
  return (
    <nav aria-label="Chat threads" className="ledger hidden md:flex flex-col w-60 shrink-0 h-full min-h-0">
      <div className="flex items-center justify-between min-h-14 pl-3.5 pr-2 shrink-0" style={{ borderBottom: "var(--hair) solid var(--border)" }}>
        <h2 className="section-title min-w-0 truncate">Conversations</h2>
        <Tooltip label="New chat">
          <button onClick={onCreate} aria-label="New chat" className="btn btn-ghost btn-icon btn-sm shrink-0">
            <Plus size={16} strokeWidth={1.75} />
          </button>
        </Tooltip>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto p-2 flex flex-col gap-0.5">
        {threads === null && ["80%", "65%", "50%"].map((w) => <div key={w} className="skeleton h-8" style={{ width: w }} />)}
        {threads?.map((t) => {
          const active = t.id === activeId;
          return (
            <div key={t.id} className="thread-row group flex items-center rounded-[3px]" data-active={active || undefined}>
              <button
                onClick={() => onSelect(t.id)}
                aria-current={active ? "true" : undefined}
                className="press flex-1 min-w-0 flex items-center gap-2 text-left min-h-10 pl-2.5 pr-2.5 text-[14px]"
              >
                <span className="truncate flex-1">{t.title}</span>
                <span className="thread-time font-mono text-[11px] shrink-0" style={{ color: "var(--ink-faint)" }}>
                  {ageLabel(t.updated_at)}
                </span>
              </button>
              <Tooltip label="Delete thread">
                <button onClick={() => onDelete(t.id)} aria-label="Delete thread" className="thread-delete btn btn-ghost btn-icon btn-sm mr-0.5 shrink-0">
                  <Trash2 size={16} strokeWidth={1.75} />
                </button>
              </Tooltip>
            </div>
          );
        })}
        {threads?.length === 0 && (
          <div className="px-2.5 py-2 text-[12px]" style={{ color: "var(--ink-faint)" }}>
            No conversations yet.
          </div>
        )}
      </div>
    </nav>
  );
}

function EmptyState({ onPick }: { onPick: (text: string) => void }) {
  return (
    <div className="flex-1 flex flex-col justify-center w-full max-w-[560px] mx-auto py-8">
      <h2 className="text-[22px] font-semibold tracking-[-0.015em]" style={{ color: "var(--ink)" }}>
        Ask your memory anything
      </h2>
      <p className="text-[15px] mt-2 mb-5 leading-relaxed" style={{ color: "var(--ink-dim)" }}>
        Ask about people, projects or documents you&apos;ve saved. Each answer shows where Eunomia looked, so you can check what it read and
        what it saved. Pick an example or type your own question below.
      </p>
      <div className="ledger hairline-rows overflow-hidden">
        {EXAMPLES.map((ex) => (
          <button key={ex} onClick={() => onPick(ex)} className="example-row press w-full flex items-center gap-2.5 min-h-11 py-2 px-4 text-left text-[14px]">
            <span className="flex-1">{ex}</span>
            <CornerDownLeft size={16} strokeWidth={1.75} className="shrink-0" style={{ color: "var(--ink-faint)" }} />
          </button>
        ))}
      </div>
    </div>
  );
}

/** Chat without a model: just what to do next, with no conversations or composer to type into. */
function NoModel({ isAdmin }: { isAdmin: boolean }) {
  return (
    <div role="status">
      <EmptyNote
        title={isAdmin ? "Set up a model to use Chat." : "Chat isn\u2019t turned on yet."}
        action={
          isAdmin && (
            <Link href="/settings?tab=model" className="btn btn-primary">
              Set up a model
            </Link>
          )
        }
      >
        {isAdmin
          ? "Chat answers questions from your saved memories and documents. It needs an OpenAI key or a local model endpoint."
          : "Ask your Eunomia admin to turn on Chat. Your connected AI apps can still look things up in the meantime."}
      </EmptyNote>
    </div>
  );
}

function LoadingTurns() {
  return (
    <div className="flex flex-col gap-7" aria-hidden>
      {["45%", "85%", "60%"].map((w) => (
        <div key={w} className="flex flex-col gap-2">
          <div className="skeleton h-5 w-20" />
          <div className="skeleton h-10" style={{ width: w }} />
        </div>
      ))}
    </div>
  );
}

export default function ChatPage() {
  const queryClient = useQueryClient();
  const threads = useThreads().data ?? null;
  const createThreadMutation = useCreateThread();
  const deleteThreadMutation = useDeleteThread();
  const [activeId, setActiveId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[] | null>(null);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [streamingText, setStreamingText] = useState("");
  const [liveSteps, setLiveSteps] = useState<ToolStep[]>([]);
  const [error, setError] = useState<Failure | null>(null);
  const [loadError, setLoadError] = useState<Failure | null>(null);
  const [needsKey, setNeedsKey] = useState(false);
  const meData = useMe().data;
  const me = meData?.email ?? null;
  // Only admins can turn chat on; members are told to ask one.
  const isAdmin = meData?.is_admin ?? false;
  // unknown while loading counts as ready: the server still refuses a send it cannot answer
  const modelReady = useSettings().data?.model_configured ?? true;
  const [lastSent, setLastSent] = useState("");
  // When the current reply was asked for, and how long it took to start answering.
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [thoughtMs, setThoughtMs] = useState<number | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const loadHistory = useCallback((threadId: string) => {
    setMessages(null);
    setLoadError(null);
    stickRef.current = true;
    queryClient
      .fetchQuery(historyQuery(threadId))
      .then(setMessages)
      .catch((e) => {
        setMessages([]);
        setLoadError(failure(e, "Could not load this conversation."));
      });
  }, [queryClient]);

  useEffect(() => {
    queryClient
      .fetchQuery({ ...threadsQuery(), staleTime: 0 })
      .then((list: ChatThread[]) => {
        // No conversations yet: start on an unsaved draft; the thread is created with the first message.
        if (list.length === 0) {
          setMessages([]);
          return;
        }
        setActiveId(list[0].id);
        loadHistory(list[0].id);
      })
      .catch((e) => {
        setMessages([]);
        setLoadError(failure(e, "Could not load your conversations."));
      });
  }, [loadHistory, queryClient]);

  // Follow new content only while the reader is at the bottom; scrolling up
  // to reread something holds the view still.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (stickRef.current) el.scrollTop = el.scrollHeight;
    edgeFade(el);
  }, [messages, sending, streamingText, liveSteps, error]);

  function selectThread(id: string) {
    if (id === activeId) return;
    setActiveId(id);
    setError(null);
    setNeedsKey(false);
    loadHistory(id);
  }

  /** "New chat" opens an unsaved draft; nothing is stored until its first message is sent. */
  function createThread() {
    setActiveId(null);
    setMessages([]);
    setError(null);
    setNeedsKey(false);
    inputRef.current?.focus();
  }

  async function deleteThread(id: string) {
    await deleteThreadMutation.mutateAsync(id);
    const remaining = (threads ?? []).filter((t) => t.id !== id);
    if (id !== activeId) return;
    if (remaining.length > 0) {
      setActiveId(remaining[0].id);
      loadHistory(remaining[0].id);
    } else {
      createThread();
    }
  }

  // `retry` resends the message already on screen instead of adding it again.
  const abortRef = useRef<AbortController | null>(null);

  async function send(raw: string = input, retry = false) {
    const text = raw.trim();
    if (!text || sending || !modelReady) return;
    if (!retry) setInput("");
    setError(null);
    setNeedsKey(false);
    setLastSent(text);
    stickRef.current = true;
    if (!retry) setMessages((prev) => [...(prev ?? []), { role: "user", content: text, created_at: new Date().toISOString() }]);
    setSending(true);
    let threadId = activeId;
    if (!threadId) {
      try {
        threadId = (await createThreadMutation.mutateAsync(undefined)).id;
        setActiveId(threadId);
      } catch (e) {
        setError(failure(e, "Could not start a new conversation."));
        setSending(false);
        return;
      }
    }
    setStreamingText("");
    setLiveSteps([]);
    const start = performance.now();
    setStartedAt(start);
    setThoughtMs(null);
    const steps: ToolStep[] = [];
    const controller = new AbortController();
    abortRef.current = controller;
    let reply = "";
    try {
      await chat.send(threadId, text, (event) => {
        if (event.type === "text") {
          if (!reply) setThoughtMs(performance.now() - start);
          reply += event.delta;
          setStreamingText(reply);
        } else if (event.type === "tool_call") {
          steps.push({ name: event.name, args: "", running: true });
          setLiveSteps([...steps]);
        } else if (event.type === "tool_result") {
          const step = steps.find((s) => s.running && s.name === event.name);
          if (step) step.running = false;
          setLiveSteps([...steps]);
        } else if (event.type === "error") {
          // code and trace id are optional: older backends send only the message
          setError(failure(event.code ? new ApiError(200, event.code, event.message, event.trace_id) : new Error(event.message), event.message));
          setNeedsKey(/OpenAI API key/i.test(event.message));
        }
      }, controller.signal);
      if (reply || steps.length) {
        // the stream only names tools; reopening the thread loads their full arguments and results
        const tool_calls = steps.map((s, i) => ({ id: `live_${i}`, type: "function" as const, function: { name: s.name, arguments: "" } }));
        setMessages((prev) => [...(prev ?? []), { role: "assistant", content: reply, tool_calls, created_at: new Date().toISOString() }]);
      }
      queryClient.invalidateQueries({ queryKey: chatKeys.threads() });
    } catch (e) {
      if (controller.signal.aborted) {
        // stopped by the person: keep whatever streamed so far
        if (reply) setMessages((prev) => [...(prev ?? []), { role: "assistant", content: reply, created_at: new Date().toISOString() }]);
        return;
      }
      const msg = e instanceof Error ? e.message : "Something went wrong.";
      setError(failure(e, msg));
      setNeedsKey(/OpenAI API key/i.test(msg));
    } finally {
      abortRef.current = null;
      setSending(false);
      setStreamingText("");
      setLiveSteps([]);
    }
  }

  const turns = messages ? toTurns(messages) : null;
  const activeTool = liveSteps.find((s) => s.running)?.name;
  const canSend = !!input.trim() && !sending && messages !== null && modelReady;

  return (
    <TooltipGroup>
    <div className="flex flex-col gap-6 h-[calc(100dvh-6.5rem)] md:h-[calc(100dvh-3rem)] lg:h-[calc(100dvh-76px)] min-h-[560px]">
      <PageHeader
        icon={MessagesSquare}
        title="Chat"
        intro="Ask a question about your saved memories and documents. Eunomia shows where each answer came from."
      />

      {!modelReady ? (
        <NoModel isAdmin={isAdmin} />
      ) : (
      <div className="flex-1 min-h-0 flex gap-4">
        <ThreadList threads={threads} activeId={activeId} onSelect={selectThread} onCreate={createThread} onDelete={deleteThread} />

        <section aria-label="Conversation" className="ledger flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
          {/* compact thread switcher where there is no room for the list */}
          <div className="md:hidden flex items-center gap-2 p-2 shrink-0" style={{ borderBottom: "var(--hair) solid var(--border)" }}>
            <Select
              aria-label="Conversation"
              className="h-10 text-[14px] flex-1 min-w-0"
              value={activeId ?? ""}
              onChange={(id) => id && selectThread(id)}
              options={[...(activeId ? [] : [{ value: "", label: "New chat" }]), ...(threads ?? []).map((t) => ({ value: t.id, label: t.title }))]}
            />
            <Tooltip label="New chat">
              <button onClick={createThread} aria-label="New chat" className="btn btn-icon">
                <Plus size={18} strokeWidth={1.75} />
              </button>
            </Tooltip>
            {activeId && (
              <Tooltip label="Delete thread">
                <button onClick={() => deleteThread(activeId)} aria-label="Delete thread" className="btn btn-ghost btn-icon">
                  <Trash2 size={18} strokeWidth={1.75} />
                </button>
              </Tooltip>
            )}
          </div>

          <div
            ref={scrollRef}
            onScroll={(e) => {
              const el = e.currentTarget;
              stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
              edgeFade(el);
            }}
            className="transcript flex-1 min-h-0 overflow-y-auto"
          >
            <div className="max-w-[720px] mx-auto px-4 md:px-6 py-6 flex flex-col gap-7 min-h-full" aria-live="polite" aria-busy={sending}>
              {loadError && <ErrorLine error={loadError} />}
              {turns === null && <LoadingTurns />}
              {turns !== null && turns.length === 0 && !sending && !loadError && modelReady && <EmptyState onPick={(text) => send(text)} />}
              {turns?.map((t, i) => <TurnView key={i} turn={t} me={me} />)}
              {sending && (
                <article className="fade-in">
                  <TurnHeader name={ASSISTANT}>
                    <span className="flex items-center gap-1.5 text-[12px]" style={{ color: "var(--ink-faint)" }}>
                      {streamingText ? (
                        <span>
                          Thought for <span className="font-mono tabular-nums">{secs(thoughtMs ?? 0)}</span>
                        </span>
                      ) : (
                        <>
                          <span className="breathe">{activeTool ? `Using ${activeTool}…` : "Thinking…"}</span>
                          {startedAt !== null && <Elapsed since={startedAt} />}
                        </>
                      )}
                    </span>
                  </TurnHeader>
                  <ToolSteps steps={liveSteps} live />
                  {streamingText && (
                    <div className="settle text-[14px] px-0.5 break-words" style={{ color: "var(--ink)" }}>
                      <Markdown text={streamingText} />
                    </div>
                  )}
                </article>
              )}
              {error && (
                <div
                  role="alert"
                  className="flex items-start gap-2.5 px-4 py-3 rounded-[2px] text-[14px]"
                  style={{ background: "var(--critical-soft)", color: "var(--critical)", borderLeft: "3px solid var(--bauhaus-app-red)" }}
                >
                  <AlertCircle size={18} strokeWidth={1.75} className="shrink-0 mt-px" aria-hidden />
                  <div className="min-w-0 break-words">
                    <div className="font-medium">The assistant couldn&apos;t reply.</div>
                    <div className="mt-0.5" style={{ color: "var(--ink-dim)" }}>
                      {needsKey && !isAdmin ? (
                        "Chat isn't turned on yet. Ask your Eunomia admin."
                      ) : needsKey ? (
                        <>
                          No OpenAI API key is set, so the assistant can&apos;t answer yet.{" "}
                          <Link href="/settings?tab=model" className="underline" style={{ color: "var(--accent-text)" }}>
                            Add one in Settings
                          </Link>
                          .
                        </>
                      ) : (
                        error.message
                      )}
                    </div>
                    {(error.code && !error.code.startsWith("http.")) || error.traceId ? (
                      <div className="mt-1 flex flex-wrap items-center gap-x-3 font-mono text-[12px]">
                        {error.code && !error.code.startsWith("http.") && <code>{error.code}</code>}
                        {error.traceId && <span>Trace {error.traceId}</span>}
                      </div>
                    ) : null}
                    {!needsKey && lastSent && (
                      <button type="button" onClick={() => send(lastSent, true)} className="btn btn-sm mt-2.5">
                        <RotateCcw size={13} strokeWidth={1.75} /> Retry
                      </button>
                    )}
                  </div>
                </div>
              )}
            </div>
          </div>

          <form
            className="shrink-0 px-3 pb-3 md:px-4 md:pb-4"
            onSubmit={(e) => {
              e.preventDefault();
              send();
            }}
          >
            <div className="field max-w-[720px] mx-auto flex flex-col cursor-text" onClick={() => inputRef.current?.focus()}>
              <textarea
                ref={inputRef}
                aria-label="Message"
                rows={1}
                className="composer w-full resize-none bg-transparent outline-none px-4 pt-3.5 pb-1 text-[15px] leading-relaxed"
                placeholder={modelReady ? "Ask a question…" : "Set up a model to chat"}
                disabled={!modelReady}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    send();
                  }
                }}
              />
              <div className="flex items-center gap-2 pl-3.5 pr-2 pb-2">
                <span className="hidden sm:flex items-center gap-1 text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  <span className="kbd">Enter</span> send <span className="kbd ml-1.5">Shift Enter</span> new line
                </span>
                <div className="ml-auto flex">
                  {sending ? (
                    <Tooltip label="Stop">
                      <button type="button" onClick={() => abortRef.current?.abort()} aria-label="Stop" className="btn">
                        <Square size={12} strokeWidth={2.5} fill="currentColor" aria-hidden /> Stop
                      </button>
                    </Tooltip>
                  ) : (
                    <Tooltip label="Send" shortcut="Enter">
                      <button type="submit" disabled={!canSend} aria-label="Send" className="btn btn-primary">
                        Send <ArrowUp size={16} strokeWidth={2} aria-hidden />
                      </button>
                    </Tooltip>
                  )}
                </div>
              </div>
            </div>
          </form>
        </section>
      </div>
      )}

      <style>{`
        .composer { field-sizing: content; min-height: 24px; max-height: 200px; }
        .thread-row { color: var(--ink); transition: background-color var(--dur-hover) ease, color var(--dur-hover) ease; }
        .thread-row[data-active] { background: var(--accent-soft); box-shadow: inset 3px 0 0 var(--bauhaus-app-blue); font-weight: 600; }
        .thread-delete { display: none; color: var(--ink-dim); }
        .thread-row:focus-within .thread-delete { display: inline-flex; }
        .thread-row:focus-within .thread-time { display: none; }
        .example-row { color: var(--ink); }
        .example-row:active { background: var(--surface-raised); }
        .press { transition: transform var(--dur-press) var(--ease-out), background-color var(--dur-hover) ease, color var(--dur-hover) ease; }
        .press:active { transform: scale(0.97); }
        .chev { transition: transform var(--dur-hover) var(--ease-out); }
        .tool-row[open] .chev { transform: rotate(90deg); }
        .tool-pre {
          font: 11.5px/1.55 var(--font-mono), ui-monospace, monospace;
          color: var(--ink);
          background: var(--surface-raised);
          border: 1px solid var(--border);
          border-radius: 2px;
          padding: 8px 10px;
          max-height: 240px;
          overflow: auto;
          white-space: pre-wrap;
          word-break: break-word;
        }
        @media (hover: hover) and (pointer: fine) {
          .thread-row:hover { background: var(--surface-raised); color: var(--ink); }
          .thread-row:hover .thread-delete { display: inline-flex; }
          .thread-row:hover .thread-time { display: none; }
          .example-row:hover { background: var(--surface-raised); color: var(--ink); }
          .tool-row summary:hover { background: var(--surface-raised); }
        }
        /* "Thinking" breathes while the assistant works; the words never change under it. */
        .breathe { animation: chat-breathe 1.6s var(--ease-in-out) infinite; }
        @keyframes chat-breathe { 50% { opacity: 0.6; } }
        /* live tool rows arrive; history rows render in place */
        .tool-live { transition: opacity 180ms var(--ease-out), transform 180ms var(--ease-out); }
        @starting-style { .tool-live { opacity: 0; transform: translateY(-4px); } }
        /* the streamed answer settles into focus once per reply */
        .settle { transition: opacity 200ms var(--ease-out), filter 200ms var(--ease-out); }
        @starting-style { .settle { opacity: 0.001; filter: blur(2px); } }
        /* static edge fade, only on a side with more to scroll (mask reads alpha, so the colour is irrelevant) */
        .transcript { --fade-top: 0px; --fade-bottom: 0px; }
        .transcript[data-fade-top] { --fade-top: 24px; }
        .transcript[data-fade-bottom] { --fade-bottom: 24px; }
        .transcript[data-fade-top],
        .transcript[data-fade-bottom] {
          mask-image: linear-gradient(to bottom, transparent, black var(--fade-top), black calc(100% - var(--fade-bottom)), transparent);
        }
        @media (prefers-reduced-motion: reduce) {
          .breathe { animation: none; }
          .tool-live, .settle { transition: none; }
        }
      `}</style>
    </div>
    </TooltipGroup>
  );
}
