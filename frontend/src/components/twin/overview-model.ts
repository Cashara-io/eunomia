// Store only a local route, never protected names or object values. Reads reauthorise it.
export const returnKey = (space: string) => `ontology-return:${space}`;
export function objectReturn(space: string): string | null {
  try {
    const value = sessionStorage.getItem(returnKey(space));
    if (!value) return null;
    const url = new URL(value, window.location.origin);
    if (
      url.origin !== window.location.origin ||
      !/^\/ontology\/objects\/[^/]+$/.test(url.pathname) ||
      url.searchParams.get("space") !== space
    )
      return null;
    decodeURIComponent(url.pathname.split("/").at(-1) ?? "");
    return url.pathname + url.search;
  } catch {
    return null;
  }
}
