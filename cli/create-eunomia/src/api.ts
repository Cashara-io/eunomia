/** The few Eunomia REST routes init and sync use. Auth is a Bearer token or a session cookie. */

export interface Vault {
  id: string;
  name: string;
  kind: string;
  role?: string;
}

/** One row of a vault's change log: what changed, never its content. */
export interface Change {
  seq: number;
  op: string;
  kind: string;
  id: string;
  actor: string;
  at: string;
}

/** Aborts when either does (`AbortSignal.any` is Node 20+; the floor is 18). */
function anySignal(a: AbortSignal, b: AbortSignal): AbortSignal {
  const ctl = new AbortController();
  for (const s of [a, b]) {
    if (s.aborted) ctl.abort(s.reason);
    else s.addEventListener("abort", () => ctl.abort(s.reason), { once: true });
  }
  return ctl.signal;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export class Api {
  /** `Bearer <token>` or a `Cookie` header; set by `login` or the constructor. */
  private auth: Record<string, string> = {};

  constructor(
    readonly url: string,
    token?: string,
  ) {
    if (token) this.useToken(token);
  }

  useToken(token: string): void {
    this.auth = { authorization: `Bearer ${token}` };
  }

  /** `cancel` aborts the request too (watch's Ctrl-C); every request gives up after 20 s. */
  async request<T>(method: string, path: string, body?: unknown, cancel?: AbortSignal): Promise<{ status: number; data: T; headers: Headers }> {
    const signal = cancel ? anySignal(cancel, AbortSignal.timeout(20_000)) : AbortSignal.timeout(20_000);
    let res: Response;
    try {
      res = await fetch(this.url + path, {
        method,
        headers: { ...this.auth, ...(body === undefined ? {} : { "content-type": "application/json" }) },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal,
        // never follow: a redirect would resend the login body (email and password) to another host
        redirect: "manual",
      });
    } catch (e) {
      throw new ApiError(0, "client.transport", `could not reach ${this.url} (${(e as Error).message})`);
    }
    if (res.status >= 300 && res.status < 400) {
      throw new ApiError(
        res.status,
        "client.redirect",
        `${this.url} answered with a redirect (HTTP ${res.status}); nothing is sent on to another address: pass the server's own address with --url`,
      );
    }
    const text = await res.text();
    let data: any = undefined;
    try {
      data = text ? JSON.parse(text) : undefined;
    } catch {
      data = text;
    }
    if (!res.ok) {
      const detail = (data && typeof data === "object" && (data.detail || data.title)) || `HTTP ${res.status}`;
      throw new ApiError(res.status, (data && data.code) || "", String(detail));
    }
    return { status: res.status, data, headers: res.headers };
  }

  async healthy(): Promise<boolean> {
    try {
      await this.request("GET", "/healthz");
      return true;
    } catch {
      return false;
    }
  }

  /** `password`, `both` or `oidc`. */
  async authMode(): Promise<string> {
    const { data } = await this.request<{ auth_mode?: string }>("GET", "/api/auth/bootstrap");
    return data?.auth_mode ?? "password";
  }

  /** Signs in with a password and keeps the session cookie for the next calls. */
  async login(email: string, password: string): Promise<void> {
    const { headers } = await this.request("POST", "/api/auth/login", { email, password });
    const m = /eunomia_session=([^;,\s]+)/.exec(headers.get("set-cookie") ?? "");
    if (!m) throw new ApiError(0, "client.no_session", "the server did not start a session");
    this.auth = { cookie: `eunomia_session=${m[1]}` };
  }

  /** Ends the session started by `login`; best effort. */
  async logout(): Promise<void> {
    if (!this.auth.cookie) return;
    await this.request("POST", "/api/auth/logout").catch(() => undefined);
  }

  /** Who the credential belongs to; a 401 means it is not valid. */
  async me(): Promise<{ id: string; email: string }> {
    return (await this.request<{ id: string; email: string }>("GET", "/api/auth/me")).data;
  }

  async listVaults(): Promise<Vault[]> {
    return (await this.request<{ results: Vault[] }>("GET", "/api/vaults")).data.results;
  }

  async createVault(name: string): Promise<Vault> {
    return (await this.request<Vault>("POST", "/api/vaults", { name, kind: "org" })).data;
  }

  async deleteToken(id: string): Promise<void> {
    await this.request("DELETE", `/api/auth/tokens/${encodeURIComponent(id)}`);
  }

  /** The narrowest token that still syncs skills and writes memory: `memory:write`, this vault only. */
  async mintProjectToken(name: string, vaultId: string): Promise<{ id: string; token: string }> {
    const body = { name, scopes: ["memory:write"], vault_id: vaultId };
    return (await this.request<{ id: string; token: string }>("POST", "/api/auth/tokens", body)).data;
  }

  /** A page of the vault's change log after `after`, oldest first. */
  async changes(vaultId: string, after: number, limit: number, cancel?: AbortSignal): Promise<{ changes: Change[]; next: number }> {
    return (await this.request<{ changes: Change[]; next: number }>("GET", `/api/vaults/${encodeURIComponent(vaultId)}/changes?after=${after}&limit=${limit}`, undefined, cancel)).data;
  }

  /** Calls a registry tool over REST. Tool failures come back as `{error, code}` with HTTP 200. */
  async tool<T = any>(name: string, args: Record<string, unknown>): Promise<T> {
    const { data } = await this.request<any>("POST", `/api/tools/${name}`, args);
    if (data && typeof data === "object" && typeof data.error === "string") {
      throw new ApiError(200, data.code ?? "", data.error);
    }
    return data as T;
  }
}
