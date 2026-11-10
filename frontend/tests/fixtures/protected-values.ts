import type { Page } from "@playwright/test";

export async function protectedValues(page: Page, names: string[]) {
  return page.evaluate(names => {
    const textarea = document.createElement("textarea");
    function decode(value: string): string {
      for (;;) {
        const previous = value;
        textarea.innerHTML = value;
        value = textarea.value.replaceAll("+", " ");
        // A literal percent in another attribute must not prevent decoding.
        value = value.replace(/(?:%[\da-f]{2})+/gi, encoded => {
          try { return decodeURIComponent(encoded); } catch { return encoded; }
        });
        if (value === previous) return value;
      }
    }
    const attributes = Array.from(document.querySelectorAll("*")).flatMap(el =>
      Array.from(el.attributes).filter(a => !(el instanceof HTMLInputElement && el.type === "search" && a.name === "value")).map(a => a.value));
    // The current URL and search input value are the user's input. All other
    // DOM text and attributes, including nested href queries, are checked.
    return [document.documentElement.textContent ?? "", ...attributes].map(decode)
      .filter(value => names.some(name => value.includes(name)));
  }, names);
}
