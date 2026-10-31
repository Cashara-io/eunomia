// 1Password service accounts, through the official SDK (@1password/sdk, MIT). The SDK talks to
// 1Password's servers itself (it holds the account unlock flow), so this is a thin adapter. A client is
// made per request from the request's token and dropped with it: nothing is kept between requests.
// The SDK is loaded lazily, so a sidecar with no service account configured never loads its WASM core.

import type { Ref } from "../refs.js";
import { ProviderFailure, type Check, type Outcome, type Provider } from "./types.js";

/** The slice of the SDK this adapter uses; tests pass a fake. */
export type SdkClient = {
  secrets: { resolveAll(refs: string[]): Promise<{ individualResponses: Record<string, { content?: { secret: string }; error?: { type: string } }> }> };
  vaults: { list(): Promise<unknown[]> };
};
export type SdkFactory = (token: string) => Promise<SdkClient>;

export const officialSdk: SdkFactory = async (token) => {
  const sdk = await import("@1password/sdk");
  return (await sdk.createClient({ auth: token, integrationName: "Eunomia secrets-helper", integrationVersion: "v1" })) as unknown as SdkClient;
};

const NOT_FOUND = new Set(["fieldNotFound", "vaultNotFound", "itemNotFound", "noMatchingSections", "tooManyVaults", "tooManyItems", "tooManyMatchingFields"]);

/** The SDK's errors are plain Errors with 1Password's text; map the ones that mean "this token". */
function failure(e: unknown): ProviderFailure {
  const text = e instanceof Error ? e.message : String(e);
  if (/invalid.*token|unauthori[sz]ed|authenticat|forbidden|revoked|expired/i.test(text)) return new ProviderFailure("denied", "1Password refused the service account token");
  if (/rate.?limit|too many requests/i.test(text)) return new ProviderFailure("provider_error", "1Password rate limited the service account");
  return new ProviderFailure("provider_error", `the 1Password SDK failed: ${text.slice(0, 200)}`);
}

export class ServiceAccountProvider implements Provider {
  constructor(
    private readonly token: string,
    private readonly sdk: SdkFactory = officialSdk,
  ) {
    if (!token.startsWith("ops_")) throw new ProviderFailure("bad_config", "a 1Password service account token starts with ops_");
  }

  private async client(signal: AbortSignal): Promise<SdkClient> {
    return withSignal(this.sdk(this.token), signal).catch((e) => {
      throw e instanceof ProviderFailure ? e : failure(e);
    });
  }

  async check(signal: AbortSignal): Promise<Check> {
    try {
      const vaults = await withSignal((await this.client(signal)).vaults.list(), signal).catch((e) => {
        throw e instanceof ProviderFailure ? e : failure(e);
      });
      return { ok: true, detail: `${vaults.length} vault${vaults.length === 1 ? "" : "s"} visible` };
    } catch (e) {
      if (e instanceof ProviderFailure) return { ok: false, error: e.error, message: e.message };
      throw e;
    }
  }

  async resolve(refs: Ref[], signal: AbortSignal): Promise<Outcome[]> {
    const wanted = refs.map((r) => (r.scheme === "op" ? toString(r) : undefined));
    const asked = wanted.filter((w): w is string => w !== undefined);
    const client = await this.client(signal);
    const res = asked.length
      ? await withSignal(client.secrets.resolveAll(asked), signal).catch((e) => {
          throw e instanceof ProviderFailure ? e : failure(e);
        })
      : { individualResponses: {} };
    return wanted.map((w): Outcome => {
      if (w === undefined) return { ok: false, error: "invalid_ref", message: "a 1Password reference starts with op://" };
      const r = res.individualResponses[w];
      if (r?.content && typeof r.content.secret === "string") return { ok: true, value: r.content.secret };
      const type = r?.error?.type ?? "unknown";
      if (NOT_FOUND.has(type)) return { ok: false, error: "not_found", message: `1Password: ${type}` };
      if (type === "parsing") return { ok: false, error: "invalid_ref", message: "1Password could not parse the reference" };
      return { ok: false, error: "provider_error", message: `1Password: ${type}` };
    });
  }
}

/** The reference as 1Password writes it (the parser accepts the same text). */
function toString(r: Extract<Ref, { scheme: "op" }>): string {
  const path = [r.vault, r.item, ...(r.section !== undefined ? [r.section] : []), r.field].join("/");
  return `op://${path}${r.attribute ? `?attribute=${r.attribute}` : ""}`;
}

/** Rejects when the request is aborted (its time budget ran out), even if the SDK call does not. */
function withSignal<T>(p: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(new ProviderFailure("provider_error", "timed out"));
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => reject(new ProviderFailure("provider_error", "timed out"));
    signal.addEventListener("abort", onAbort, { once: true });
    p.then(
      (v) => {
        signal.removeEventListener("abort", onAbort);
        resolve(v);
      },
      (e) => {
        signal.removeEventListener("abort", onAbort);
        reject(e);
      },
    );
  });
}
