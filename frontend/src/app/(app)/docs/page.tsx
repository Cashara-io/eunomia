"use client";

import Select from "@/components/Select";
import { isValidElement, useEffect, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { ArrowLeft, ArrowRight, BookOpen, Link2 } from "lucide-react";
import PageHeader from "@/components/ui/PageHeader";
import { useDoc, useDocs } from "@/lib/queries/tools";
import ErrorLine, { failure } from "@/components/ErrorLine";
import CopyButton from "@/components/bits/CopyButton";
import Tooltip from "@/components/bits/Tooltip";

type Topic = { topic: string; title: string };

// A short, people-first start page that lives here rather than in docs/: what someone does day to day.
const USING_TOPIC = "using";
const USING_TITLE = "Using Eunomia";
const USING_MARKDOWN = `# Using Eunomia

Eunomia is your shared memory. Your AI app looks things up in it and saves what is worth keeping, so you don't have to repeat yourself.

## Using Eunomia with ChatGPT

1. On **Home**, choose ChatGPT under *Which AI app do you use?* and follow the steps. Other apps (Claude, Codex and more) work the same way.
2. In a new ChatGPT conversation, type \`@\` and choose Eunomia, then ask your question, for example *What do we know about the billing rewrite?*
3. Ask it to remember things too: *Remember that Priya owns the billing rewrite.*

If ChatGPT can't reach Eunomia, the steps say so. Ask your Eunomia admin to give the server a public address.

## Finding what's saved

- **Home** shows what was updated recently across every space you belong to.
- **People & topics** shows who and what is saved, and how they connect. Choose a space at the top.
- **Chat** answers questions from your saved memories and documents, when your admin has turned it on.
- **Documents** holds uploaded files, so AI apps can quote them.

## Memory spaces

Everything is saved in a space. **Personal** is yours alone; a team space is shared with everyone in it. When someone invites you, the invitation appears on Home. See [How Eunomia works](concepts.md) for how spaces work.

## Changing what your AI remembers

Admins can add **AI instructions**: the instructions AI apps follow, for example *always remember project deadlines*. If there is something your AI app should always remember, ask your admin.
`;

/** Help topics, people first: reading and using Eunomia, then installation and running it. */
const ADMIN_TOPICS = new Set(["quickstart", "installation", "deployment"]);

/** Plain names for topics whose served title is jargon. Used for the nav and the page title. */
const PLAIN_TITLES: Record<string, string> = { agents: "Connecting AI apps", concepts: "How Eunomia works" };
const plainTitle = (t: Topic): Topic => ({ ...t, title: PLAIN_TITLES[t.topic] ?? t.title });

function textOf(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement(node)) return textOf((node.props as { children?: ReactNode }).children);
  return "";
}

// ponytail: duplicate headings in one doc share an id; the first one wins the anchor.
const slugify = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^\w\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-");

function CodeBlock({ children }: { children?: ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  return (
    <div className="relative">
      <pre ref={ref} className="pr-11">
        {children}
      </pre>
      <div className="absolute top-1.5 right-1.5 rounded-[4px]" style={{ background: "var(--surface)" }}>
        <Tooltip label="Copy code">
          <CopyButton label="Copy code" value={() => ref.current?.innerText.trimEnd() ?? ""} />
        </Tooltip>
      </div>
    </div>
  );
}

function Heading({ level, children }: { level: 2 | 3; children?: ReactNode }) {
  const Tag = `h${level}` as const;
  const id = slugify(textOf(children));
  return (
    <Tag id={id} className="group scroll-mt-6">
      {children}
      <a
        href={`#${id}`}
        aria-hidden
        tabIndex={-1}
        className="ml-2 inline-flex align-middle opacity-0 transition-opacity duration-150 group-hover:opacity-100"
        style={{ color: "var(--ink-dim)", textDecoration: "none" }}
      >
        <Link2 size={14} strokeWidth={1.75} />
      </a>
    </Tag>
  );
}

// The repo's docs/ folder, served by the same `docs` tool agents call over MCP.
export default function DocsPage() {
  const topicsQuery = useDocs();
  const served: Topic[] = (topicsQuery.data?.docs ?? []).map(plainTitle);
  const using: Topic[] = served.length > 0 ? [{ topic: USING_TOPIC, title: USING_TITLE }, ...served.filter((t) => !ADMIN_TOPICS.has(t.topic))] : [];
  const forAdmins: Topic[] = served.filter((t) => ADMIN_TOPICS.has(t.topic));
  const groups = [
    { label: "Using Eunomia", topics: using },
    { label: "For admins", topics: forAdmins },
  ].filter((g) => g.topics.length > 0);
  const topics: Topic[] = [...using, ...forAdmins];
  const [topic, setTopic] = useState(USING_TOPIC);
  const local = topic === USING_TOPIC;
  // The start page is local; meanwhile fetch the next topic people usually open.
  const docQuery = useDoc(local ? "agents" : topic);
  const body = local ? USING_MARKDOWN : (docQuery.data?.markdown ?? null);
  const error = !local && docQuery.isError
    ? failure(docQuery.error, "Could not load that help page.", " Check that Eunomia is running, then reload the page.")
    : topicsQuery.isError
      ? failure(topicsQuery.error, "Could not load the help pages.", " Check that Eunomia is running, then reload the page.")
      : null;
  const [outline, setOutline] = useState<{ id: string; text: string }[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const articleRef = useRef<HTMLElement>(null);
  const first = useRef(true);

  useEffect(() => {
    if (first.current) first.current = false;
    else window.scrollTo({ top: 0 });
  }, [topic]);

  // "On this page": read the h2s back out of the rendered article.
  useEffect(() => {
    const el = articleRef.current;
    if (!el || body === null) return;
    const hs = [...el.querySelectorAll<HTMLHeadingElement>("h2[id]")];
    setOutline(hs.map((h) => ({ id: h.id, text: h.textContent ?? "" })));
    setActive(hs[0]?.id ?? null);
    const io = new IntersectionObserver(
      (entries) => {
        const hit = entries.find((e) => e.isIntersecting);
        if (hit) setActive(hit.target.id);
      },
      { rootMargin: "0px 0px -70% 0px" },
    );
    hs.forEach((h) => io.observe(h));
    return () => io.disconnect();
  }, [body]);

  // The doc's own "# Title" becomes the page title, so the page has one h1.
  const docTitle = PLAIN_TITLES[topic] ?? body?.match(/^#\s+(.+)$/m)?.[1]?.trim() ?? topics.find((t) => t.topic === topic)?.title ?? "Help";

  const components: Components = {
    h1: () => null,
    h2: ({ children }) => <Heading level={2}>{children}</Heading>,
    h3: ({ children }) => <Heading level={3}>{children}</Heading>,
    pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
    a: ({ href = "", children, title }) => {
      const props = { children, title };
      const doc = href.match(/^([\w-]+)\.md(#.*)?$/);
      if (doc)
        return (
          <a
            {...props}
            href={`#${doc[1]}`}
            onClick={(e) => {
              e.preventDefault();
              setTopic(doc[1]);
            }}
          />
        );
      if (href.startsWith("#")) return <a {...props} href={href} />;
      return <a {...props} href={href} target="_blank" rel="noopener noreferrer" />;
    },
  };

  const index = topics.findIndex((t) => t.topic === topic);
  const prev = index > 0 ? topics[index - 1] : null;
  const next = index >= 0 && index < topics.length - 1 ? topics[index + 1] : null;

  return (
    <div className="flex flex-col gap-7 max-w-[1200px]">
    <PageHeader icon={BookOpen} title={docTitle} />
    <div className="grid gap-5 md:grid-cols-[184px_minmax(0,1fr)] md:gap-10 xl:grid-cols-[184px_minmax(0,760px)_184px]">
      <nav aria-label="Help topics" className="md:sticky md:top-8 md:self-start">
        {topics.length === 0 && (
          <ul className="hidden md:flex flex-col gap-1">
            {[64, 80, 72, 92, 60].map((w) => (
              <li key={w} className="h-10 flex items-center px-3">
                <span className="skeleton h-3" style={{ width: `${w}%` }} />
              </li>
            ))}
          </ul>
        )}
        <div className="hidden md:flex flex-col gap-5">
          {groups.map((g) => (
            <div key={g.label} className="flex flex-col gap-1">
              {/* No group label that would just repeat the page title ("Using Eunomia" on its own page). */}
              {g.label !== docTitle && (
                <h2 className="text-[12px] font-semibold px-3" style={{ color: "var(--ink-dim)" }}>
                  {g.label}
                </h2>
              )}
              <ul className="flex flex-col gap-1">
                {g.topics.map((t) => {
                  const current = t.topic === topic;
                  return (
                    <li key={t.topic}>
                      <button
                        onClick={() => setTopic(t.topic)}
                        aria-current={current ? "page" : undefined}
                        className={`w-full text-left min-h-10 px-3 rounded-[4px] text-[14px] truncate transition-colors duration-150 ${current ? "font-semibold" : "hover:bg-[var(--bauhaus-app-soft)]"}`}
                        style={current ? { background: "var(--bauhaus-app-blue)", color: "#fff" } : { color: "var(--ink)" }}
                      >
                        {t.title}
                      </button>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </div>
        <div className="md:hidden flex flex-col gap-1.5">
          <span className="text-[14px] font-medium" aria-hidden>Help topic</span>
          <Select
            aria-label="Help topic"
            className="h-11 text-[14px] w-full"
            value={topic}
            onChange={setTopic}
            placeholder="Loading help"
            options={groups.flatMap((g) => g.topics.map((t) => ({ value: t.topic, label: t.title, hint: g.label === "For admins" ? "For admins" : undefined })))}
          />
        </div>
      </nav>

      <div className="min-w-0 flex flex-col gap-4">
        <article ref={articleRef} className="text-[15px] leading-[1.7] docs">
          {error ? (
            <ErrorLine error={error} />
          ) : body === null ? (
            <div className="flex flex-col gap-3" aria-busy="true" aria-label="Loading help page">
              <span className="skeleton h-7 w-48 mb-3" />
              {[100, 94, 88, 97, 60].map((w) => (
                <span key={w} className="skeleton h-3.5" style={{ width: `${w}%` }} />
              ))}
            </div>
          ) : (
            <div className="md">
              <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
                {body}
              </ReactMarkdown>
            </div>
          )}
        </article>

        {(prev || next) && (
          <div className="grid grid-cols-2 gap-3 pt-4" style={{ borderTop: "1px solid var(--border)" }}>
            {prev ? (
              <button onClick={() => setTopic(prev.topic)} className="btn h-auto py-2.5 flex-col items-start gap-1 text-left min-w-0 font-medium">
                <span className="label inline-flex items-center gap-1">
                  <ArrowLeft size={12} strokeWidth={1.75} /> Previous
                </span>
                <span className="truncate max-w-full">{prev.title}</span>
              </button>
            ) : (
              <span />
            )}
            {next && (
              <button onClick={() => setTopic(next.topic)} className="btn h-auto py-2.5 flex-col items-end gap-1 text-right min-w-0 font-medium">
                <span className="label inline-flex items-center gap-1">
                  Next <ArrowRight size={12} strokeWidth={1.75} />
                </span>
                <span className="truncate max-w-full">{next.title}</span>
              </button>
            )}
          </div>
        )}
      </div>

      {outline.length > 1 && (
        <aside className="hidden xl:block sticky top-8 self-start" aria-label="On this page">
          <p className="text-[12px] font-semibold mb-2 pl-3" style={{ color: "var(--ink-dim)" }}>
            On this page
          </p>
          <ul className="flex flex-col text-[13px]" style={{ borderLeft: "1px solid var(--border)" }}>
            {outline.map((h) => (
              <li key={h.id}>
                <a
                  href={`#${h.id}`}
                  className="block py-1 pl-3 -ml-px transition-colors duration-150"
                  style={{
                    borderLeft: `2px solid ${active === h.id ? "var(--bauhaus-app-blue)" : "transparent"}`,
                    color: active === h.id ? "var(--ink)" : "var(--ink-dim)",
                  }}
                >
                  {h.text}
                </a>
              </li>
            ))}
          </ul>
        </aside>
      )}
    </div>
    </div>
  );
}
