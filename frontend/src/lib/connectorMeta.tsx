import {
  Calendar,
  CheckSquare,
  CreditCard,
  Database,
  GitBranch,
  Landmark,
  Mail,
  MessageCircle,
  MessagesSquare,
  Mic,
  Music,
  Notebook,
  Workflow,
} from "lucide-react";
import type { Connector } from "@/lib/types";

export type FieldDef = { key: string; label: string; placeholder: string; secret: boolean };

export type ConnectorMeta = {
  label: string;
  description: string;
  icon: React.ReactNode;
  /** CSS custom property name carrying this connector's icon-tile tint. */
  tint: string;
  /** Where to get the credential and which scopes it needs. */
  help?: React.ReactNode;
  fields: FieldDef[];
  /** `sources.list()` key this connector's data lives under. */
  sourceKey: string;
  /** This connector's provider supports push webhooks -- shows the per-user webhook URL to register with it. */
  webhooks: boolean;
};

function A({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <a href={href} target="_blank" rel="noreferrer" className="underline" style={{ color: "var(--accent-text)" }}>
      {children}
    </a>
  );
}

function Steps({ children }: { children: React.ReactNode }) {
  return <ol className="list-decimal pl-5 flex flex-col gap-1">{children}</ol>;
}

const C = ({ children }: { children: React.ReactNode }) => <code className="font-mono break-all">{children}</code>;

// Google and Spotify have no personal tokens: you bring your own OAuth client
// and a refresh token, which the server exchanges for a fresh access token on
// every sync.
const OAUTH_FIELDS: FieldDef[] = [
  { key: "client_id", label: "Client ID", placeholder: "…", secret: false },
  { key: "client_secret", label: "Client secret", placeholder: "…", secret: true },
  { key: "refresh_token", label: "Refresh token", placeholder: "…", secret: true },
];

function googleHelp(api: string, scope: string) {
  return (
    <Steps>
      <li>
        In <A href="https://console.cloud.google.com/apis/library">Google Cloud Console</A>, enable the {api}.
      </li>
      <li>
        Configure the OAuth consent screen (External) and add yourself as a test user. Publish the app (&ldquo;In
        production&rdquo;) or the refresh token expires after 7 days.
      </li>
      <li>
        Create an OAuth client ID of type <em>Web application</em> with redirect URI{" "}
        <C>https://developers.google.com/oauthplayground</C>.
      </li>
      <li>
        In the <A href="https://developers.google.com/oauthplayground">OAuth 2.0 Playground</A>, open ⚙, &ldquo;Use your
        own OAuth credentials&rdquo;, authorize the scope <C>{scope}</C>, then &ldquo;Exchange authorization code for
        tokens&rdquo; and copy the refresh token.
      </li>
    </Steps>
  );
}

export const CONNECTOR_META: Record<Connector["kind"], ConnectorMeta> = {
  up_bank: {
    label: "Up Bank",
    description: "Transactions, accounts and categories from your Up Bank account.",
    icon: <Landmark size={18} strokeWidth={1.75} />,
    tint: "var(--connector-up-bank)",
    help: (
      <>
        Generate a personal access token at <A href="https://api.up.com.au/getting_started">api.up.com.au</A> (or in the
        Up app: Profile, Data sharing). It is read-only. The webhook secret is optional, only needed if you register
        the webhook URL below with Up.
      </>
    ),
    fields: [
      { key: "personal_access_token", label: "Personal access token", placeholder: "up:yeah:…", secret: true },
      { key: "webhook_secret_key", label: "Webhook secret key (optional)", placeholder: "the secretKey Up gave you", secret: true },
    ],
    sourceKey: "up_bank",
    webhooks: true,
  },
  pocketai: {
    label: "PocketAI",
    description: "Meeting recordings with transcripts and summaries from HeyPocket.",
    icon: <Mic size={18} strokeWidth={1.75} />,
    tint: "var(--connector-pocketai)",
    help: (
      <>
        Create an API key in your <A href="https://heypocketai.com">Pocket</A> account&apos;s developer settings. Leave
        the base URL empty unless you were given a different API host.
      </>
    ),
    fields: [
      { key: "api_key", label: "API key", placeholder: "pk_…", secret: true },
      { key: "base_url", label: "Base URL (optional)", placeholder: "https://public.heypocketai.com/api/v1", secret: false },
    ],
    sourceKey: "heypocket",
    webhooks: false,
  },
  github: {
    label: "GitHub",
    description: "Issues and pull requests you created, are assigned to, or are mentioned in.",
    icon: <GitBranch size={18} strokeWidth={1.75} />,
    tint: "var(--connector-github)",
    help: (
      <>
        Create a <A href="https://github.com/settings/personal-access-tokens/new">fine-grained token</A> with read-only
        access to <em>Issues</em> and <em>Pull requests</em> on the repositories you want (or a{" "}
        <A href="https://github.com/settings/tokens/new?scopes=repo">classic token</A> with the <C>repo</C> scope). The
        first sync reads the last 90 days.
      </>
    ),
    fields: [{ key: "personal_access_token", label: "Personal access token", placeholder: "github_pat_… or ghp_…", secret: true }],
    sourceKey: "github",
    webhooks: false,
  },
  slack: {
    label: "Slack",
    description: "Messages in the channels your Slack app has joined.",
    icon: <MessagesSquare size={18} strokeWidth={1.75} />,
    tint: "var(--connector-slack)",
    help: (
      <Steps>
        <li>
          Create an app at <A href="https://api.slack.com/apps">api.slack.com/apps</A> (From scratch).
        </li>
        <li>
          OAuth &amp; Permissions, Bot Token Scopes: <C>channels:read</C>, <C>channels:history</C>, <C>groups:read</C>,{" "}
          <C>groups:history</C>.
        </li>
        <li>Install it to your workspace and copy the Bot User OAuth Token.</li>
        <li>
          In Slack, <C>/invite @your-app</C> to each channel to sync. The first sync reads the last 30 days.
        </li>
      </Steps>
    ),
    fields: [{ key: "bot_token", label: "Bot token", placeholder: "xoxb-…", secret: true }],
    sourceKey: "slack",
    webhooks: false,
  },
  notion: {
    label: "Notion",
    description: "Pages (with their text) and databases shared with your integration.",
    icon: <Notebook size={18} strokeWidth={1.75} />,
    tint: "var(--connector-notion)",
    help: (
      <>
        Create an internal integration at <A href="https://www.notion.so/profile/integrations">notion.so/profile/integrations</A>{" "}
        with the &ldquo;Read content&rdquo; capability and copy its secret. Then, on each page or database to sync, open
        •••, Connections and add the integration.
      </>
    ),
    fields: [{ key: "integration_token", label: "Integration secret", placeholder: "ntn_… or secret_…", secret: true }],
    sourceKey: "notion",
    webhooks: false,
  },
  linear: {
    label: "Linear",
    description: "Issues assigned to you.",
    icon: <Workflow size={18} strokeWidth={1.75} />,
    tint: "var(--connector-linear)",
    help: (
      <>
        Create a personal API key in <A href="https://linear.app/settings/account/security">Linear, Settings, Security &amp; access</A>{" "}
        (read access is enough).
      </>
    ),
    fields: [{ key: "api_key", label: "API key", placeholder: "lin_api_…", secret: true }],
    sourceKey: "linear",
    webhooks: false,
  },
  gmail: {
    label: "Gmail",
    description: "Your email from the last 30 days onwards, with subject, sender and text.",
    icon: <Mail size={18} strokeWidth={1.75} />,
    tint: "var(--connector-gmail)",
    help: googleHelp("Gmail API", "https://www.googleapis.com/auth/gmail.readonly"),
    fields: OAUTH_FIELDS,
    sourceKey: "gmail",
    webhooks: false,
  },
  google_calendar: {
    label: "Google Calendar",
    description: "Events on your primary calendar, 30 days back to 6 months ahead, kept up to date.",
    icon: <Calendar size={18} strokeWidth={1.75} />,
    tint: "var(--connector-google-calendar)",
    help: googleHelp("Google Calendar API", "https://www.googleapis.com/auth/calendar.readonly"),
    fields: OAUTH_FIELDS,
    sourceKey: "google_calendar",
    webhooks: false,
  },
  discord: {
    label: "Discord",
    description: "Messages from the channels you choose, read by your own bot.",
    icon: <MessageCircle size={18} strokeWidth={1.75} />,
    tint: "var(--connector-discord)",
    help: (
      <Steps>
        <li>
          In the <A href="https://discord.com/developers/applications">Developer Portal</A>, create an application, Bot ,
          Reset Token and copy it; enable <em>Message Content Intent</em>.
        </li>
        <li>
          OAuth2, URL Generator: scope <C>bot</C>, permissions <em>View Channels</em> + <em>Read Message History</em>;
          open the URL to add the bot to your server.
        </li>
        <li>
          Channel IDs: turn on Developer Mode (User Settings, Advanced), right-click a channel, Copy Channel ID.
          Separate several with commas.
        </li>
      </Steps>
    ),
    fields: [
      { key: "bot_token", label: "Bot token", placeholder: "…", secret: true },
      { key: "channel_id", label: "Channel IDs", placeholder: "123456789012345678, …", secret: false },
    ],
    sourceKey: "discord",
    webhooks: false,
  },
  spotify: {
    label: "Spotify",
    description: "Tracks you play, building up a listening history over time.",
    icon: <Music size={18} strokeWidth={1.75} />,
    tint: "var(--connector-spotify)",
    help: (
      <Steps>
        <li>
          Create an app at <A href="https://developer.spotify.com/dashboard">developer.spotify.com/dashboard</A> (Web API)
          with redirect URI <C>http://127.0.0.1:8888/callback</C>.
        </li>
        <li>
          Open{" "}
          <C>
            https://accounts.spotify.com/authorize?response_type=code&amp;scope=user-read-recently-played&amp;redirect_uri=http://127.0.0.1:8888/callback&amp;client_id=CLIENT_ID
          </C>
          , approve, and copy the <C>code</C> from the address bar (the page itself won&apos;t load).
        </li>
        <li>
          Run{" "}
          <C>
            curl -u CLIENT_ID:CLIENT_SECRET -d grant_type=authorization_code -d code=CODE -d
            redirect_uri=http://127.0.0.1:8888/callback https://accounts.spotify.com/api/token
          </C>{" "}
          and paste its <C>refresh_token</C> here.
        </li>
      </Steps>
    ),
    fields: OAUTH_FIELDS,
    sourceKey: "spotify",
    webhooks: false,
  },
  todoist: {
    label: "Todoist",
    description: "Your active tasks, with project, due date and labels.",
    icon: <CheckSquare size={18} strokeWidth={1.75} />,
    tint: "var(--connector-todoist)",
    help: (
      <>
        Copy your API token from <A href="https://app.todoist.com/app/settings/integrations/developer">Todoist, Settings ,
        Integrations, Developer</A>.
      </>
    ),
    fields: [{ key: "api_token", label: "API token", placeholder: "…", secret: true }],
    sourceKey: "todoist",
    webhooks: false,
  },
  stripe: {
    label: "Stripe",
    description: "Charges on your Stripe account.",
    icon: <CreditCard size={18} strokeWidth={1.75} />,
    tint: "var(--connector-stripe)",
    help: (
      <>
        Create a <A href="https://dashboard.stripe.com/apikeys">restricted key</A> with <em>Charges: Read</em> and no
        other permissions, not your full secret key.
      </>
    ),
    fields: [{ key: "secret_key", label: "Restricted key", placeholder: "rk_live_…", secret: true }],
    sourceKey: "stripe",
    webhooks: false,
  },
};

export const CONNECTOR_ORDER: Connector["kind"][] = [
  "up_bank",
  "pocketai",
  "github",
  "slack",
  "notion",
  "linear",
  "gmail",
  "google_calendar",
  "discord",
  "spotify",
  "todoist",
  "stripe",
];

export function connectorStatus(c: Connector | undefined): "demo" | "connected" | "disconnected" {
  if (!c) return "disconnected";
  if (c.config?.demo) return "demo";
  if (c.enabled && c.credentials_set) return "connected";
  return "disconnected";
}

/** Connector kind whose data lives under a `sources.list()` key, if any. An added connection's key is `{source}~{instance}`. */
export function kindForSource(sourceKey: string): Connector["kind"] | undefined {
  const base = sourceKey.split("~")[0];
  return CONNECTOR_ORDER.find((k) => CONNECTOR_META[k].sourceKey === base);
}

/** Short relative age for a sync timestamp: "4m ago", "never". */
export function relativeTime(iso: string | null | undefined): string {
  if (!iso) return "never";
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

/** Connector identity: a small icon tile in the connector's own tint, never the accent. */
export function ConnectorTile({ kind, size = 28 }: { kind: Connector["kind"] | null; size?: number }) {
  const meta = kind ? CONNECTOR_META[kind] : null;
  const tint = meta?.tint ?? "var(--ink-faint)";
  return (
    <span
      aria-hidden
      className="inline-flex items-center justify-center shrink-0"
      style={{
        width: size,
        height: size,
        borderRadius: size >= 40 ? 10 : 7,
        color: tint,
        background: `color-mix(in oklab, ${tint} 16%, var(--surface))`,
        boxShadow: `inset 0 0 0 1px color-mix(in oklab, ${tint} 32%, transparent)`,
      }}
    >
      <span className="inline-flex" style={{ transform: `scale(${Math.max(0.75, size / 34)})` }}>
        {meta?.icon ?? <Database size={18} strokeWidth={1.75} />}
      </span>
    </span>
  );
}
