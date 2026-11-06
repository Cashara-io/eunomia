// Bitwarden Secrets Manager with a machine account access token, over Bitwarden's public endpoints:
//   POST <identity>/connect/token  (client credentials: client_id = token id, client_secret, scope api.secrets)
//        -> {access_token, encrypted_payload}; the payload, decrypted with the token key, is
//           {"encryptionKey": "<base64 64-byte organisation key>"}
//   GET  <api>/secrets/{id}  (Bearer access_token) -> {id, organizationId, key, value, ...}, value an
//        EncString under the organisation key
// Server: the US cloud by default; https://vault.bitwarden.eu for the EU cloud; any other URL is a
// self-hosted server with /identity and /api under it.

import type { Ref } from "../refs.js";
import { decrypt, DecryptError, keyFromB64, parseAccessToken, TokenError, tokenKey, type SymKey } from "./bitwarden_crypto.js";
import { getJson, ProviderFailure, type Check, type Outcome, type Provider } from "./types.js";

const CLOUDS: Record<string, { identity: string; api: string }> = {
  "https://vault.bitwarden.com": { identity: "https://identity.bitwarden.com", api: "https://api.bitwarden.com" },
  "https://vault.bitwarden.eu": { identity: "https://identity.bitwarden.eu", api: "https://api.bitwarden.eu" },
};

export function endpoints(host?: string): { identity: string; api: string } {
  if (!host) return CLOUDS["https://vault.bitwarden.com"]!;
  const url = new URL(host);
  if (url.protocol !== "https:" && url.protocol !== "http:") throw new ProviderFailure("bad_config", "the Bitwarden server is an http(s) URL");
  const base = url.origin + url.pathname.replace(/\/+$/, "");
  return CLOUDS[base] ?? { identity: `${base}/identity`, api: `${base}/api` };
}

export class BitwardenProvider implements Provider {
  private readonly token;
  private readonly urls;

  constructor(token: string, host?: string) {
    try {
      this.token = parseAccessToken(token);
    } catch (e) {
      throw new ProviderFailure("bad_config", e instanceof TokenError ? e.message : "not a Bitwarden access token");
    }
    this.urls = endpoints(host);
  }

  /** Logs in and returns the bearer token and the organisation key. */
  private async login(signal: AbortSignal): Promise<{ bearer: string; org: SymKey }> {
    const form = new URLSearchParams({ scope: "api.secrets", grant_type: "client_credentials", client_id: this.token.id, client_secret: this.token.secret });
    const { status, body } = await getJson(`${this.urls.identity}/connect/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json", "device-type": "21" },
      body: form.toString(),
      signal,
    });
    if (status === 400 || status === 401 || status === 403) throw new ProviderFailure("denied", `Bitwarden refused the access token (HTTP ${status})`);
    if (status < 200 || status >= 300) throw new ProviderFailure("provider_error", `Bitwarden identity answered HTTP ${status}`);
    const b = body as { access_token?: unknown; encrypted_payload?: unknown } | undefined;
    if (typeof b?.access_token !== "string" || typeof b.encrypted_payload !== "string") throw new ProviderFailure("provider_error", "Bitwarden identity sent an unexpected answer");
    let payload: { encryptionKey?: unknown };
    try {
      payload = JSON.parse(decrypt(b.encrypted_payload, tokenKey(this.token.seed)).toString("utf8"));
    } catch (e) {
      throw new ProviderFailure("denied", e instanceof DecryptError ? "the access token's key does not open Bitwarden's answer" : "Bitwarden's answer did not decrypt");
    }
    if (typeof payload.encryptionKey !== "string") throw new ProviderFailure("provider_error", "Bitwarden's answer has no organisation key");
    return { bearer: b.access_token, org: keyFromB64(payload.encryptionKey) };
  }

  async check(signal: AbortSignal): Promise<Check> {
    try {
      await this.login(signal);
      return { ok: true, detail: "signed in as the machine account" };
    } catch (e) {
      if (e instanceof ProviderFailure) return { ok: false, error: e.error, message: e.message };
      throw e;
    }
  }

  async resolve(refs: Ref[], signal: AbortSignal): Promise<Outcome[]> {
    const { bearer, org } = await this.login(signal);
    const out: Outcome[] = [];
    for (const ref of refs) {
      if (ref.scheme !== "bws") {
        out.push({ ok: false, error: "invalid_ref", message: "a Bitwarden reference is bws://<secret id>" });
        continue;
      }
      const { status, body } = await getJson(`${this.urls.api}/secrets/${ref.id}`, { headers: { authorization: `Bearer ${bearer}`, accept: "application/json" }, signal });
      if (status === 404) {
        out.push({ ok: false, error: "not_found", message: "no such secret" });
        continue;
      }
      if (status === 401 || status === 403) {
        out.push({ ok: false, error: "denied", message: `Bitwarden refused this secret to the machine account (HTTP ${status})` });
        continue;
      }
      const value = (body as { value?: unknown } | undefined)?.value;
      if (status < 200 || status >= 300 || typeof value !== "string") {
        out.push({ ok: false, error: "provider_error", message: `Bitwarden answered HTTP ${status}` });
        continue;
      }
      try {
        out.push({ ok: true, value: decrypt(value, org).toString("utf8") });
      } catch {
        out.push({ ok: false, error: "provider_error", message: "the secret did not decrypt with the organisation key" });
      }
    }
    return out;
  }
}
