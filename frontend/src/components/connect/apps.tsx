import { Asterisk, LayoutGrid, MessageCircle, SquareTerminal } from "lucide-react";
import type { ApiToken, OAuthGrant } from "@/lib/types";

export type AppId = "chatgpt" | "claude-code" | "codex" | "other";

export type AiApp = {
  id: AppId;
  name: string;
  /** One line under the name in the picker: what this app is, so people pick the right one. */
  hint: string;
  /** Matches the name an app registers with when it signs in (OAuth) or the API token named after it. */
  match: RegExp;
  /** Where to go to use it, once connected. */
  openUrl?: string;
};

export const AI_APPS: AiApp[] = [
  { id: "chatgpt", name: "ChatGPT", hint: "Web or desktop app", match: /chatgpt|openai/i, openUrl: "https://chatgpt.com" },
  { id: "claude-code", name: "Claude Code", hint: "Coding terminal, not Claude chat", match: /claude/i },
  { id: "codex", name: "Codex", hint: "Coding app or terminal", match: /codex/i },
  { id: "other", name: "Other apps", hint: "Claude chat, Cursor and more", match: /./ },
];

export const appById = (id: string): AiApp | undefined => AI_APPS.find((a) => a.id === id);

/** A recognisable symbol for each app. These are category symbols drawn with
 * Lucide, not the vendors' official logos (none are bundled with the app);
 * the visible name always sits beside them. */
export function AppMark({ id, size = 24 }: { id: AppId; size?: number }) {
  const s = { width: size, height: size };
  if (id === "claude-code")
    return (
      <span className="inline-flex items-center justify-center shrink-0" style={{ ...s, color: "#c15f3c" }} aria-hidden>
        <Asterisk size={size} strokeWidth={2.5} />
      </span>
    );
  if (id === "codex")
    return (
      <span className="inline-flex items-center justify-center shrink-0 rounded-full" style={{ ...s, background: "var(--bauhaus-app-ink)", color: "#fff" }} aria-hidden>
        <SquareTerminal size={Math.round(size * 0.62)} strokeWidth={2} />
      </span>
    );
  if (id === "chatgpt")
    return (
      <span className="inline-flex items-center justify-center shrink-0" style={s} aria-hidden>
        <MessageCircle size={size} strokeWidth={1.75} />
      </span>
    );
  return (
    <span className="inline-flex items-center justify-center shrink-0" style={s} aria-hidden>
      <LayoutGrid size={size} strokeWidth={1.75} />
    </span>
  );
}

const isLiveToken = (t: ApiToken) => t.last_used_at !== null && (t.expires_at === null || new Date(t.expires_at).getTime() > Date.now());

export type Connection = { name: string; via: "sign-in" | "token"; at: string | null };

/** Connections the server has confirmed: an approved sign-in (OAuth grant), or
 * an API token an app has actually used. A copied command or a token nobody
 * used yet is not proof of anything. Newest first. */
export function confirmedConnections(grants: OAuthGrant[] | undefined, tokens: ApiToken[] | undefined): Connection[] {
  const fromGrants: Connection[] = (grants ?? []).map((g) => ({ name: g.client_name, via: "sign-in", at: g.last_used_at ?? g.created_at }));
  const fromTokens: Connection[] = (tokens ?? []).filter(isLiveToken).map((t) => ({ name: t.name, via: "token", at: t.last_used_at }));
  return [...fromGrants, ...fromTokens].sort((a, b) => (b.at ?? "").localeCompare(a.at ?? ""));
}

/** The confirmed connection for one app, if any. "Other apps" counts anything
 * that is not ChatGPT, Claude Code or Codex. */
export function connectionFor(app: AiApp, all: Connection[]): Connection | undefined {
  if (app.id !== "other") return all.find((c) => app.match.test(c.name));
  const named = AI_APPS.filter((a) => a.id !== "other");
  return all.find((c) => !named.some((a) => a.match.test(c.name)));
}

/** Which picker entry a connection belongs to. */
export function appForConnection(c: Connection): AiApp {
  return AI_APPS.find((a) => a.id !== "other" && a.match.test(c.name)) ?? AI_APPS[3];
}
