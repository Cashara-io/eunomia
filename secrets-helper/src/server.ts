// The sidecar's internal API, version 1 (docs/architecture/specs/broker.md, "Sidecar internal API").
//   GET  /healthz     no auth
//   POST /v1/resolve  {provider: {kind, token, host?}, refs: [...]} -> {results: [...]}
//   POST /v1/check    {provider: {kind, token, host?}} -> {ok, detail | error, message}
// Stateless: the token comes in each request and is dropped with it. Nothing here logs a body, a token
// or a value; every message goes through the request's Scrubber first.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { timingSafeEqual, createHash } from "node:crypto";
import { parseRef, RefError, type Ref } from "./refs.js";
import { Scrubber } from "./scrub.js";
import { BitwardenProvider } from "./providers/bitwarden.js";
import { ConnectProvider } from "./providers/connect.js";
import { ServiceAccountProvider, officialSdk, type SdkFactory } from "./providers/service_account.js";
import { KINDS, ProviderFailure, type Kind, type Outcome, type Provider, type ProviderConfig } from "./providers/types.js";

export const API_VERSION = 1;
export const MAX_BODY = 64 * 1024;
export const MAX_REFS = 16;
export const MAX_TOKEN = 4096;

export type Options = {
  /** SECRETS_HELPER_KEY; empty: no key check (the internal network is the only gate). */
  key: string;
  /** Per request, in ms (the spec's 10 s). */
  timeoutMs?: number;
  /** For tests: the service account SDK. */
  sdk?: SdkFactory;
  /** Where operational lines go (never a body, token or value). */
  log?: (line: string) => void;
  version?: string;
};

class BadRequest extends Error {}

function send(res: ServerResponse, status: number, body: unknown) {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store", "content-length": Buffer.byteLength(text) });
  res.end(text);
}

async function readBody(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    size += (chunk as Buffer).length;
    if (size > MAX_BODY) throw Object.assign(new Error("too large"), { status: 413 });
    chunks.push(chunk as Buffer);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new BadRequest("the body is not JSON");
  }
}

function keyOk(header: string | undefined, key: string): boolean {
  if (!key) return true;
  const given = header?.startsWith("Bearer ") ? header.slice(7) : "";
  // hash both so the compare is constant time whatever the lengths
  const a = createHash("sha256").update(given).digest();
  const b = createHash("sha256").update(key).digest();
  return timingSafeEqual(a, b) && given.length > 0;
}

function config(body: unknown): ProviderConfig {
  const p = (body as { provider?: unknown } | null)?.provider as Record<string, unknown> | undefined;
  if (!p || typeof p !== "object") throw new BadRequest("provider is required");
  if (typeof p.kind !== "string" || !KINDS.includes(p.kind as Kind)) throw new BadRequest(`provider.kind is one of ${KINDS.join(", ")}`);
  if (typeof p.token !== "string" || p.token.length === 0 || p.token.length > MAX_TOKEN) throw new BadRequest("provider.token is required");
  if (p.host !== undefined && p.host !== null && typeof p.host !== "string") throw new BadRequest("provider.host is a string");
  const host = typeof p.host === "string" && p.host.length ? p.host : undefined;
  if (p.kind === "onepassword_connect" && !host) throw new BadRequest("provider.host (the Connect server URL) is required for onepassword_connect");
  return { kind: p.kind as Kind, token: p.token, host };
}

function provider(cfg: ProviderConfig, sdk: SdkFactory): Provider {
  try {
    switch (cfg.kind) {
      case "onepassword_connect":
        return new ConnectProvider(cfg.token, cfg.host!);
      case "onepassword_service_account":
        return new ServiceAccountProvider(cfg.token, sdk);
      case "bitwarden_sm":
        return new BitwardenProvider(cfg.token, cfg.host);
    }
  } catch (e) {
    if (e instanceof ProviderFailure) throw e;
    throw new ProviderFailure("bad_config", "the provider settings are not valid (check the host URL)");
  }
}

export function makeServer(opts: Options): Server {
  const sdk = opts.sdk ?? officialSdk;
  const log = opts.log ?? ((l: string) => process.stdout.write(`${l}\n`));
  const timeoutMs = opts.timeoutMs ?? 10_000;

  return createServer(async (req, res) => {
    const scrub = new Scrubber();
    const started = Date.now();
    const route = `${req.method} ${(req.url ?? "").split("?")[0]}`;
    let status = 500;
    try {
      if (route === "GET /healthz") {
        status = 200;
        return send(res, 200, { ok: true, api: API_VERSION, version: opts.version ?? "dev" });
      }
      if (route !== "POST /v1/resolve" && route !== "POST /v1/check") {
        status = 404;
        return send(res, 404, { error: "not_found" });
      }
      if (!keyOk(req.headers.authorization, opts.key)) {
        status = 401;
        return send(res, 401, { error: "unauthorized" });
      }
      const body = await readBody(req);
      const cfg = config(body);
      scrub.token(cfg.token);
      const signal = AbortSignal.timeout(timeoutMs);

      if (route === "POST /v1/check") {
        let out;
        try {
          out = await provider(cfg, sdk).check(signal);
        } catch (e) {
          out = e instanceof ProviderFailure ? { ok: false, error: e.error, message: e.message } : { ok: false, error: "provider_error", message: signal.aborted ? "timed out" : "the provider call failed" };
        }
        if (!out.ok) out = { ...out, message: scrub.clean(out.message) };
        status = 200;
        return send(res, 200, out);
      }

      const raw = (body as { refs?: unknown }).refs;
      if (!Array.isArray(raw) || raw.length === 0 || raw.length > MAX_REFS || !raw.every((r) => typeof r === "string")) {
        throw new BadRequest(`refs is a list of 1 to ${MAX_REFS} strings`);
      }
      const parsed: Array<Ref | RefError> = raw.map((r: string) => {
        try {
          return parseRef(r);
        } catch (e) {
          return e instanceof RefError ? e : new RefError("malformed reference");
        }
      });
      const good = parsed.filter((p): p is Ref => !(p instanceof RefError));
      let resolved: Outcome[] = [];
      let whole: Outcome | undefined;
      if (good.length) {
        try {
          resolved = await provider(cfg, sdk).resolve(good, signal);
        } catch (e) {
          const message = e instanceof ProviderFailure ? e.message : signal.aborted ? "timed out" : "the provider call failed";
          const error = e instanceof ProviderFailure && e.error === "denied" ? "denied" : "provider_error";
          whole = { ok: false, error, message };
        }
      }
      for (const r of resolved) if (r.ok) scrub.value(r.value);
      let i = 0;
      const results = raw.map((ref: string, n) => {
        const p = parsed[n]!;
        if (p instanceof RefError) return { ref, ok: false, error: "invalid_ref", message: p.message };
        const r = whole ?? resolved[i++] ?? { ok: false as const, error: "provider_error" as const, message: "no answer" };
        return r.ok ? { ref, ok: true, value: r.value } : { ref, ok: false, error: r.error, message: scrub.clean(r.message) };
      });
      status = 200;
      return send(res, 200, { results });
    } catch (e) {
      if (e instanceof BadRequest) {
        status = 400;
        return send(res, 400, { error: "bad_request", message: scrub.clean(e.message) });
      }
      if ((e as { status?: number }).status === 413) {
        status = 413;
        return send(res, 413, { error: "too_large" });
      }
      status = 500;
      log(`error ${route}: ${scrub.clean(e instanceof Error ? e.message : String(e)).slice(0, 300)}`);
      return send(res, 500, { error: "internal" });
    } finally {
      if (route !== "GET /healthz") log(`${route} ${status} ${Date.now() - started}ms`);
    }
  });
}
