// 1Password Connect: the REST API of the customer's own Connect server (Bearer = the Connect token).
//   GET /v1/vaults                       -> [{id, name}]
//   GET /v1/vaults/{vault}/items         -> [{id, title}]
//   GET /v1/vaults/{vault}/items/{item}  -> {id, title, sections: [{id, label}], fields: [{id, label, value, section: {id}}]}
// Vaults, items, sections and fields match by id or, case-insensitively, by name (as op:// does).

import type { OpRef, Ref } from "../refs.js";
import { getJson, ProviderFailure, type Check, type Outcome, type Provider } from "./types.js";

type Named = { id?: string; name?: string; title?: string; label?: string };
type Field = Named & { value?: string; section?: { id?: string }; type?: string };
type Item = Named & { sections?: Named[]; fields?: Field[] };

const same = (a: string | undefined, want: string) => a !== undefined && a.toLowerCase() === want.toLowerCase();
const matches = (n: Named, want: string) => n.id === want || same(n.name ?? n.title ?? n.label, want);

export class ConnectProvider implements Provider {
  private readonly base: string;

  constructor(
    private readonly token: string,
    host: string,
  ) {
    const url = new URL(host); // throws on a malformed host: the caller reports bad_config
    if (url.protocol !== "https:" && url.protocol !== "http:") throw new ProviderFailure("bad_config", "the Connect host is an http(s) URL");
    this.base = url.origin + url.pathname.replace(/\/+$/, "");
  }

  private async get(path: string, signal: AbortSignal): Promise<unknown> {
    const { status, body } = await getJson(this.base + path, { headers: { authorization: `Bearer ${this.token}`, accept: "application/json" }, signal });
    if (status === 401 || status === 403) throw new ProviderFailure("denied", `the Connect server refused the token (HTTP ${status})`);
    if (status === 404) return undefined;
    if (status < 200 || status >= 300) throw new ProviderFailure("provider_error", `the Connect server answered HTTP ${status}`);
    return body;
  }

  private async list(path: string, signal: AbortSignal): Promise<Named[]> {
    const body = await this.get(path, signal);
    if (body !== undefined && !Array.isArray(body)) throw new ProviderFailure("provider_error", "the Connect server sent an unexpected list");
    return (body as Named[] | undefined) ?? [];
  }

  async check(signal: AbortSignal): Promise<Check> {
    try {
      const vaults = await this.list("/v1/vaults", signal);
      return { ok: true, detail: `${vaults.length} vault${vaults.length === 1 ? "" : "s"} visible` };
    } catch (e) {
      if (e instanceof ProviderFailure) return { ok: false, error: e.error, message: e.message };
      throw e;
    }
  }

  async resolve(refs: Ref[], signal: AbortSignal): Promise<Outcome[]> {
    const vaults = await this.list("/v1/vaults", signal);
    const items = new Map<string, Named[]>();
    const full = new Map<string, Item | undefined>();
    const out: Outcome[] = [];
    for (const ref of refs) {
      if (ref.scheme !== "op") {
        out.push({ ok: false, error: "invalid_ref", message: "a 1Password Connect reference starts with op://" });
        continue;
      }
      out.push(await this.one(ref, vaults, items, full, signal));
    }
    return out;
  }

  private async one(ref: OpRef, vaults: Named[], items: Map<string, Named[]>, full: Map<string, Item | undefined>, signal: AbortSignal): Promise<Outcome> {
    if (ref.attribute && ref.attribute !== "value") return { ok: false, error: "invalid_ref", message: `the attribute ${JSON.stringify(ref.attribute)} is not supported through Connect` };
    const vs = vaults.filter((v) => matches(v, ref.vault));
    if (vs.length !== 1) return { ok: false, error: "not_found", message: vs.length ? "more than one vault matches" : "no such vault" };
    const vid = vs[0]!.id!;
    if (!items.has(vid)) items.set(vid, await this.list(`/v1/vaults/${encodeURIComponent(vid)}/items`, signal));
    const is = items.get(vid)!.filter((i) => matches(i, ref.item));
    if (is.length !== 1) return { ok: false, error: "not_found", message: is.length ? "more than one item matches" : "no such item" };
    const key = `${vid}/${is[0]!.id}`;
    if (!full.has(key)) full.set(key, (await this.get(`/v1/vaults/${encodeURIComponent(vid)}/items/${encodeURIComponent(is[0]!.id!)}`, signal)) as Item | undefined);
    const item = full.get(key);
    if (!item) return { ok: false, error: "not_found", message: "no such item" };
    let fields = item.fields ?? [];
    if (ref.section !== undefined) {
      const sections = (item.sections ?? []).filter((s) => matches(s, ref.section!));
      if (sections.length !== 1) return { ok: false, error: "not_found", message: sections.length ? "more than one section matches" : "no such section" };
      fields = fields.filter((f) => f.section?.id === sections[0]!.id);
    }
    let hits = fields.filter((f) => matches(f, ref.field));
    // op://vault/item/field: a field outside any section wins over same-named ones inside sections
    if (ref.section === undefined && hits.length > 1) {
      const top = hits.filter((f) => !f.section?.id);
      if (top.length === 1) hits = top;
    }
    if (hits.length !== 1) return { ok: false, error: "not_found", message: hits.length ? "more than one field matches" : "no such field" };
    const value = hits[0]!.value;
    if (typeof value !== "string" || value.length === 0) return { ok: false, error: "not_found", message: "the field is empty" };
    return { ok: true, value };
  }
}
