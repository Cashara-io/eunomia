// Domain types the pages use. They narrow the generated client's looser
// shapes (string kinds become unions); the query hooks cast to them.

export type Me = { id: string; email: string; onboarded: boolean; has_password: boolean; oidc_provider?: string | null };

export type Scope = "memory:read" | "memory:write" | "vaults:admin" | "connectors";
export type ApiToken = {
  id: string;
  name: string;
  created_at: string | null;
  last_used_at: string | null;
  scopes: Scope[];
  /** Set when the token is restricted to one vault. */
  vault_id: string | null;
  /** Null for a token that never expires. */
  expires_at: string | null;
};
export type Session = { id: string; user_agent: string; created_at: string | null; last_seen_at: string | null; current: boolean };

export type OAuthGrant = {
  id: string;
  client_id: string;
  client_name: string;
  client_logo: string | null;
  scope: string[];
  created_at: string;
  last_used_at: string | null;
};

export type AppSettings = {
  embedding_model: string;
  /** "" = automatic (OPENAI_CHAT_MODEL, else gpt-4o-mini on OpenAI, else the endpoint's first chat model). */
  chat_model: string;
  sync_intervals: Record<string, number>;
  theme: { mode?: "light" | "dark" | "system"; accent?: string };
  openai_api_key_set: boolean;
  /** A usable model endpoint resolves for this user, so chat can answer. */
  model_configured: boolean;
  openai_base_url: string;
  observations_mission: string;
  memory_skill: string;
  memory_skill_custom: boolean;
};

export type OpenAiModels = { models: string[]; error: string | null };

export type UpdateStatus =
  | { configured: false }
  | {
      configured: true;
      current_version: string;
      latest_version: string;
      update_available: boolean;
      checked_at: string;
      applying: boolean;
      error: string | null;
      /** While applying: what the updater is waiting on. Absent from updaters older than this field. */
      progress?: string | null;
    };

export type HttpsStatus =
  | { configured: false }
  | {
      configured: true;
      state: "off" | "pending" | "active" | "error";
      domain?: string;
      message?: string | null;
      checked_at?: string;
    };

export type ConnectorKind =
  | "up_bank"
  | "pocketai"
  | "github"
  | "slack"
  | "notion"
  | "linear"
  | "gmail"
  | "google_calendar"
  | "discord"
  | "spotify"
  | "todoist"
  | "stripe";

/** One routing rule: a record whose `field` matches `value` goes to `vault`. */
export type RoutingRule = { field: string; value: string; vault: string };
/** Where a connector files its records: `default_vault` (absent: personal) unless a rule matches first. */
export type Routing = { default_vault?: string; rules: RoutingRule[] };

/** One connection. A kind can be connected more than once (two Gmail accounts). */
export type Connector = {
  /** `connector:...`; routes taking a connector accept it. */
  id: string;
  kind: ConnectorKind;
  /** The name the user gave this connection; "" if none. */
  label: string;
  /** Its key in /api/sources and for syncing: the source key for a kind's first connection, `{source}~{instance}` for an added one. */
  source_key: string;
  enabled: boolean;
  config: Record<string, unknown>;
  credentials_set: boolean;
  updated_at: string | null;
  routing: Routing;
  /** The fields this connector's rules can match on. */
  routing_fields: string[];
};

export type ConnectorUpdate = Partial<{
  enabled: boolean;
  label: string;
  config: Record<string, unknown>;
  credentials: Record<string, string>;
  routing: Routing;
}>;

/** What "Move existing data" re-filed. */
export type MoveReport = { records: number; memories: number; relations: number; left: number };

export type Snapshot = {
  up_bank: { transaction_count: number; spent: number } | null;
  pocketai: { recordings_count: number } | null;
};

export type SyncStatus = {
  cursor: string;
  last_run: string | null;
  last_ok: string | null;
  last_error: string;
  consecutive_failures: number;
};

export type SourceRow = {
  key: string;
  label: string;
  /** The connection's id; absent for a source never set up. */
  connector?: string | null;
  /** The name the user gave the connection; "" if none. */
  account?: string;
  provider: string;
  record_types: string[];
  connected: boolean;
  sync_status: SyncStatus;
  record_count: number;
};

// A record as the generic `search`/`list` tools summarise it (no payload).
/** Where a hit on an uploaded document's passage comes from (`documents::chunk_ref`). */
export type DocumentRef = {
  document_id: string;
  chunk_id: string;
  filename: string;
  revision: number;
  part: number;
  char_start: number;
  char_end: number;
  download_url: string;
};

export type ToolHit = {
  id: string;
  source: string;
  type: string;
  title: string;
  snippet: string;
  occurred_at: string | null;
  url: string | null;
  /** Set on a passage of an uploaded document. */
  document?: DocumentRef;
};

/** The documents page for a hit's document, scrolled to the passage. */
export const documentHref = (d: DocumentRef) => `/documents?id=${encodeURIComponent(d.document_id)}&chunk=${encodeURIComponent(d.chunk_id)}`;

// A record as the generic `get` tool returns it in full.
export type ToolRecord = ToolHit & {
  external_id: string;
  body_text: string;
  payload: Record<string, unknown>;
  /** A Pocket recording or transcript chunk: the whole stored recording. */
  recording?: { summary: string; action_items: string[]; transcript: string };
};

export type BuiltInKind = "person" | "organisation" | "location" | "repository" | "file" | "symbol";
/** A built-in kind, or a custom kind an agent created (a lowercase name such as "db"). */
export type EntityKind = BuiltInKind | (string & {});

export type EntitySummary = {
  id: string;
  kind: EntityKind;
  name: string;
  aliases: string[];
  summary: string;
};

export type EntityMemory = { id: string; text: string; created_at?: string; source?: string; owner_email: string | null };
export type EntityRelation = {
  id: string;
  in: string;
  out: string;
  label: string;
  direction: "in" | "out";
  owner_email: string | null;
};

export type EntityDetail = EntitySummary & {
  owner_email: string | null;
  memory: EntityMemory[];
  relations: EntityRelation[];
};

export type EntityGraphNode = { id: string; kind: EntityKind; name: string; owner_email: string | null };
export type EntityGraphEdge = { source: string; target: string; label: string; owner_email: string | null };
export type EntityGraph = { nodes: EntityGraphNode[]; edges: EntityGraphEdge[] };

export type VaultRole = "owner" | "member";
export type VaultKind = "personal" | "org";

export type Vault = { id: string; name: string; kind: VaultKind; created_at: string | null; role: VaultRole };
export type VaultMember = { email: string; role: VaultRole };
export type VaultInvitation = {
  vault_id: string;
  vault_name: string;
  vault_kind: VaultKind;
  role: VaultRole;
  created_at: string | null;
};

export type CloudPoint = { id: string; vault: string; kind: string; label: string; x: number; y: number; z: number };
export type Cloud = { space: "semantic" | "lexical"; points: CloudPoint[] };

export type ChatToolCall = { id: string; type: "function"; function: { name: string; arguments: string } };

export type ChatMessage = {
  role: "user" | "assistant" | "tool";
  content: string;
  tool_calls?: ChatToolCall[] | null;
  created_at?: string;
};

export type ChatThread = { id: string; title: string; created_at: string | null; updated_at: string | null };
