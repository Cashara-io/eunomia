import type { Ref } from "../refs.js";

export type Kind = "onepassword_connect" | "onepassword_service_account" | "bitwarden_sm";
export const KINDS: readonly Kind[] = ["onepassword_connect", "onepassword_service_account", "bitwarden_sm"];

export type ProviderConfig = { kind: Kind; token: string; host?: string };

/** Per reference: the value, or why not. */
export type Outcome = { ok: true; value: string } | { ok: false; error: "not_found" | "denied" | "invalid_ref" | "provider_error"; message: string };

export type Check = { ok: true; detail: string } | { ok: false; error: "denied" | "provider_error" | "bad_config"; message: string };

export interface Provider {
  resolve(refs: Ref[], signal: AbortSignal): Promise<Outcome[]>;
  check(signal: AbortSignal): Promise<Check>;
}

/** A provider call failed as a whole (auth, network): every reference of the request gets it. */
export class ProviderFailure extends Error {
  constructor(
    readonly error: "denied" | "provider_error" | "bad_config",
    message: string,
  ) {
    super(message);
  }
}

/** fetch with the request's abort signal and a JSON body; never follows redirects (a token must not
 * be sent on to another host). */
export async function getJson(url: string, init: RequestInit & { signal: AbortSignal }): Promise<{ status: number; body: unknown }> {
  const res = await fetch(url, { ...init, redirect: "manual" });
  const text = await res.text();
  let body: unknown = undefined;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = undefined;
  }
  return { status: res.status, body };
}
