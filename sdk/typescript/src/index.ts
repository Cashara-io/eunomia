/** TypeScript client for the Eunomia REST API. */

export interface Vault {
  id: string;
  name: string;
  kind: string;
  created_at?: string | null;
  /** Only set by `listVaults`. */
  role?: string;
}
export interface CloneResult extends Vault {
  entities_copied: number;
}
export interface MergeResult extends Vault {
  entities: number;
  merged_from: string[];
}
export interface Member {
  email: string;
  role: string;
}
export interface InviteResult {
  vault_id: string;
  user_email: string;
  role: string;
}
export interface Invitation {
  vault_id: string;
  vault_name: string;
  vault_kind: string;
  role: string;
  created_at?: string | null;
}
export interface MemoryWriteResult {
  entity: Record<string, unknown>;
  memory: Record<string, unknown>;
}
export interface RecallHit {
  id: string;
  kind: string;
  text: string;
  vault?: string;
  vault_name?: string;
  score?: number;
  [key: string]: unknown;
}
export interface Entity {
  id: string;
  kind: string;
  name: string;
  aliases?: string[];
  summary?: string;
  vault?: string;
  [key: string]: unknown;
}

export interface MemoryItem {
  subjectName: string;
  subjectKind: string;
  text: string;
  /** `world` (default), `experience` or `observation`. */
  type?: string;
  sourceRecordId?: string;
  /** A vault id or name. Overrides the vault passed to `writeMemories`. */
  vaultId?: string;
}

/** Outcome of one item in `writeMemories`: `result` on success, else `error`. */
export interface WriteOutcome {
  index: number;
  item: MemoryItem;
  result?: MemoryWriteResult;
  error?: EunomiaError;
}

/** An API error (problem+json), or a transport failure (`status` 0, `code` `client.transport`). */
export class EunomiaError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    readonly detail: string,
    readonly traceId?: string,
    readonly retryAfter?: number,
  ) {
    super(`${status} ${code}: ${detail}${traceId ? ` (trace ${traceId})` : ""}`);
    this.name = "EunomiaError";
  }
}

export interface ClientOptions {
  /** A personal API token, sent as `Authorization: Bearer`. */
  token: string;
  /** Retries of a `429`, waiting for `Retry-After`. Default 5. */
  maxRetries?: number;
  /** Longest single wait in seconds. Default 60. */
  maxWait?: number;
  /** Override `fetch` (tests, proxies). */
  fetch?: typeof fetch;
  /** Override the wait, in milliseconds (tests). */
  sleep?: (ms: number) => Promise<void>;
}

const enc = encodeURIComponent;
const realSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export class Eunomia {
  private readonly baseUrl: string;
  private readonly token: string;
  private readonly maxRetries: number;
  private readonly maxWait: number;
  private readonly fetchFn: typeof fetch;
  private readonly sleep: (ms: number) => Promise<void>;
  private resumeAt = 0;

  constructor(baseUrl: string, opts: ClientOptions) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.token = opts.token;
    this.maxRetries = opts.maxRetries ?? 5;
    this.maxWait = opts.maxWait ?? 60;
    this.fetchFn = opts.fetch ?? ((...a) => fetch(...a));
    this.sleep = opts.sleep ?? realSleep;
  }

  // -- transport --------------------------------------------------------

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    for (let attempt = 0; ; attempt++) {
      // While one request waits out a 429, every other request waits too.
      for (let d = this.resumeAt - Date.now(); d > 0; d = this.resumeAt - Date.now()) await this.sleep(d);
      let res: Response;
      try {
        res = await this.fetchFn(this.baseUrl + path, {
          method,
          headers: {
            Authorization: `Bearer ${this.token}`,
            Accept: "application/json",
            ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
          },
          body: body !== undefined ? JSON.stringify(body) : undefined,
        });
      } catch (e) {
        throw new EunomiaError(0, "client.transport", e instanceof Error ? e.message : String(e));
      }
      if (res.ok) {
        const text = await res.text();
        return (text ? JSON.parse(text) : undefined) as T;
      }
      const err = await errorFrom(res);
      if (err.status === 429 && attempt < this.maxRetries) {
        const wait = Math.min(err.retryAfter ?? 2 ** (attempt + 1), this.maxWait);
        this.resumeAt = Math.max(this.resumeAt, Date.now() + wait * 1000);
        continue;
      }
      throw err;
    }
  }

  // -- vaults -----------------------------------------------------------

  /** GET /api/vaults */
  async listVaults(): Promise<Vault[]> {
    return (await this.request<{ results: Vault[] }>("GET", "/api/vaults")).results;
  }

  /** There is no single-vault route, so this filters `listVaults` by id or name. Throws `vault.not_found`. */
  async getVault(idOrName: string): Promise<Vault> {
    const v = (await this.listVaults()).find((x) => x.id === idOrName || x.name === idOrName);
    if (!v) throw new EunomiaError(404, "vault.not_found", "Vault not found.");
    return v;
  }

  /** POST /api/vaults. `kind` is `org` (default) or `personal`. */
  createVault(name: string, opts: { kind?: "org" | "personal" } = {}): Promise<Vault> {
    return this.request("POST", "/api/vaults", { name, kind: opts.kind ?? "org" });
  }

  /** PATCH /api/vaults/{id} */
  renameVault(vaultId: string, name: string): Promise<Vault> {
    return this.request("PATCH", `/api/vaults/${enc(vaultId)}`, { name });
  }

  /** DELETE /api/vaults/{id} */
  async deleteVault(vaultId: string): Promise<void> {
    await this.request("DELETE", `/api/vaults/${enc(vaultId)}`);
  }

  /** POST /api/vaults/{id}/clone */
  cloneVault(vaultId: string, opts: { name?: string; kind?: "org" | "personal" } = {}): Promise<CloneResult> {
    return this.request("POST", `/api/vaults/${enc(vaultId)}/clone`, { kind: opts.kind ?? "org", ...(opts.name !== undefined ? { name: opts.name } : {}) });
  }

  /** POST /api/vaults/merge. Creates a new vault; the two sources are left untouched. */
  mergeVaults(vaultA: string, vaultB: string, opts: { name?: string; kind?: "org" | "personal" } = {}): Promise<MergeResult> {
    return this.request("POST", "/api/vaults/merge", { vault_ids: [vaultA, vaultB], kind: opts.kind ?? "org", ...(opts.name !== undefined ? { name: opts.name } : {}) });
  }

  /** GET /api/vaults/{id}/members */
  async listMembers(vaultId: string): Promise<Member[]> {
    return (await this.request<{ results: Member[] }>("GET", `/api/vaults/${enc(vaultId)}/members`)).results;
  }

  /** POST /api/vaults/{id}/members */
  inviteMember(vaultId: string, email: string, role = "member"): Promise<InviteResult> {
    return this.request("POST", `/api/vaults/${enc(vaultId)}/members`, { email, role });
  }

  /** DELETE /api/vaults/{id}/members/{email} */
  async removeMember(vaultId: string, email: string): Promise<void> {
    await this.request("DELETE", `/api/vaults/${enc(vaultId)}/members/${enc(email)}`);
  }

  /** POST /api/vaults/{id}/leave */
  async leaveVault(vaultId: string): Promise<void> {
    await this.request("POST", `/api/vaults/${enc(vaultId)}/leave`);
  }

  /** GET /api/vaults/invitations */
  async listInvitations(): Promise<Invitation[]> {
    return (await this.request<{ results: Invitation[] }>("GET", "/api/vaults/invitations")).results;
  }

  /** POST /api/vaults/{id}/invitations/accept */
  acceptInvitation(vaultId: string): Promise<Vault> {
    return this.request("POST", `/api/vaults/${enc(vaultId)}/invitations/accept`);
  }

  /** POST /api/vaults/{id}/invitations/decline */
  async declineInvitation(vaultId: string): Promise<void> {
    await this.request("POST", `/api/vaults/${enc(vaultId)}/invitations/decline`);
  }

  // -- memory (POST /api/tools/{name}) -----------------------------------

  /** Call any registered tool; the methods below wrap three of them. */
  callTool<T = unknown>(name: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.request("POST", `/api/tools/${enc(name)}`, args);
  }

  /** Tool `memory_write`. `vaultId` may be a vault id or its name. */
  memoryWrite(item: MemoryItem): Promise<MemoryWriteResult> {
    return this.callTool("memory_write", {
      subject_name: item.subjectName,
      subject_kind: item.subjectKind,
      text: item.text,
      type: item.type ?? "world",
      ...(item.sourceRecordId !== undefined ? { source_record_id: item.sourceRecordId } : {}),
      ...(item.vaultId !== undefined ? { vault_id: item.vaultId } : {}),
    });
  }

  /** Tool `recall`. Without a vault it searches all your vaults. */
  async recall(query: string, opts: { vaultId?: string; vaultIds?: string[]; limit?: number; types?: string[] } = {}): Promise<RecallHit[]> {
    const r = await this.callTool<{ results: RecallHit[] }>("recall", {
      query,
      limit: opts.limit ?? 20,
      ...(opts.vaultId !== undefined ? { vault_id: opts.vaultId } : {}),
      ...(opts.vaultIds !== undefined ? { vault_ids: opts.vaultIds } : {}),
      ...(opts.types !== undefined ? { types: opts.types } : {}),
    });
    return r.results;
  }

  /** Tool `entities_search`. */
  async entitiesSearch(
    query: string,
    opts: { vaultId?: string; vaultIds?: string[]; kind?: string; limit?: number; offset?: number } = {},
  ): Promise<Entity[]> {
    const r = await this.callTool<{ results: Entity[] }>("entities_search", {
      query,
      limit: opts.limit ?? 50,
      offset: opts.offset ?? 0,
      ...(opts.vaultId !== undefined ? { vault_id: opts.vaultId } : {}),
      ...(opts.vaultIds !== undefined ? { vault_ids: opts.vaultIds } : {}),
      ...(opts.kind !== undefined ? { kind: opts.kind } : {}),
    });
    return r.results;
  }

  /**
   * Write many facts with a pool of `concurrency` workers. A `429` waits for the server's
   * `Retry-After` and retries (up to `maxRetries` per item); the wait pauses every worker.
   * A failed item does not stop the run: you get one outcome per item, in input order.
   */
  async writeMemories(
    items: Iterable<MemoryItem>,
    opts: { vaultId?: string; concurrency?: number; onResult?: (o: WriteOutcome) => void } = {},
  ): Promise<WriteOutcome[]> {
    const list = [...items];
    const out: WriteOutcome[] = new Array(list.length);
    let next = 0;
    const worker = async () => {
      while (next < list.length) {
        const index = next++;
        const item = list[index];
        let o: WriteOutcome;
        try {
          o = { index, item, result: await this.memoryWrite({ ...item, vaultId: item.vaultId ?? opts.vaultId }) };
        } catch (e) {
          o = { index, item, error: e instanceof EunomiaError ? e : new EunomiaError(0, "client.error", String(e)) };
        }
        out[index] = o;
        opts.onResult?.(o);
      }
    };
    await Promise.all(Array.from({ length: Math.max(1, Math.min(opts.concurrency ?? 4, list.length || 1)) }, worker));
    return out;
  }
}

async function errorFrom(res: Response): Promise<EunomiaError> {
  const ra = Number(res.headers.get("retry-after"));
  const retryAfter = res.headers.has("retry-after") && Number.isFinite(ra) ? Math.max(0, ra) : undefined;
  const trace = res.headers.get("x-trace-id") ?? undefined;
  try {
    const p = (await res.json()) as { code?: string; detail?: string; title?: string; trace_id?: string | null };
    return new EunomiaError(res.status, p.code ?? `http.${res.status}`, p.detail ?? p.title ?? res.statusText, p.trace_id ?? trace, retryAfter);
  } catch {
    return new EunomiaError(res.status, `http.${res.status}`, res.statusText, trace, retryAfter);
  }
}
