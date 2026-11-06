// Secret references, the two schemes the broker knows (docs/architecture/specs/broker.md, "References").
//   op://<vault>/<item>/<field> and op://<vault>/<item>/<section>/<field>, 1Password's own syntax, with
//     an optional ?attribute=<name> query
//   bws://<secret-id>, ours: a Bitwarden Secrets Manager secret by its UUID
// The backend parses the same grammar (backend/src/secrets/reference.rs); keep the two in step.

export type OpRef = { scheme: "op"; vault: string; item: string; section?: string; field: string; attribute?: string };
export type BwsRef = { scheme: "bws"; id: string };
export type Ref = OpRef | BwsRef;

const MAX_PART = 256;
const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/;

export class RefError extends Error {}

export function parseRef(raw: string): Ref {
  if (typeof raw !== "string" || raw.length === 0 || raw.length > 1024) throw new RefError("a reference is a non-empty string of at most 1024 characters");
  if (CONTROL.test(raw)) throw new RefError("a reference holds no control characters");
  const scheme = raw.slice(0, raw.indexOf("://")).toLowerCase();
  const rest = raw.slice(raw.indexOf("://") + 3);
  if (raw.indexOf("://") < 0) throw new RefError("a reference starts with op:// or bws://");
  if (scheme === "bws") {
    if (!UUID.test(rest)) throw new RefError("bws:// is followed by a secret id (a UUID) and nothing else");
    return { scheme: "bws", id: rest.toLowerCase() };
  }
  if (scheme !== "op") throw new RefError("a reference starts with op:// or bws://");
  const q = rest.indexOf("?");
  const path = q < 0 ? rest : rest.slice(0, q);
  let attribute: string | undefined;
  if (q >= 0) {
    const params = new URLSearchParams(rest.slice(q + 1));
    const keys = [...params.keys()];
    if (keys.length !== 1 || keys[0] !== "attribute" || !params.get("attribute")) throw new RefError("the only query an op:// reference takes is ?attribute=<name>");
    attribute = params.get("attribute")!.toLowerCase();
  }
  const parts = path.split("/");
  if (parts.length !== 3 && parts.length !== 4) throw new RefError("op:// takes vault/item/field or vault/item/section/field");
  for (const p of parts) {
    if (p.trim().length === 0) throw new RefError("every part of an op:// reference is non-empty");
    if (p.length > MAX_PART) throw new RefError(`each part of an op:// reference is at most ${MAX_PART} characters`);
  }
  const [vault, item, a, b] = parts as [string, string, string, string?];
  return b === undefined ? { scheme: "op", vault, item, field: a, attribute } : { scheme: "op", vault, item, section: a, field: b, attribute };
}
