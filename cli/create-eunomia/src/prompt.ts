/** Terminal questions over node:readline, with hidden input for passwords. */

import { createInterface, type Interface } from "node:readline";

export interface Prompter {
  ask(question: string, fallback?: string): Promise<string>;
  secret(question: string): Promise<string>;
  confirm(question: string, fallback: boolean): Promise<boolean>;
  close(): void;
}

export function terminalPrompter(): Prompter {
  let rl: Interface | undefined;
  let muted = false;
  const open = () => {
    if (!rl) {
      rl = createInterface({ input: process.stdin, output: process.stdout, terminal: Boolean(process.stdin.isTTY) });
      // hide typed characters while `muted`, keep the newline
      const write = (rl as any)._writeToOutput?.bind(rl);
      (rl as any)._writeToOutput = (s: string) => {
        if (!muted) write?.(s);
        else if (s.includes("\n")) process.stdout.write("\n");
      };
    }
    return rl;
  };
  const question = (q: string) => new Promise<string>((resolve) => open().question(q, resolve));
  return {
    async ask(q, fallback) {
      const a = (await question(fallback ? `${q} [${fallback}]: ` : `${q}: `)).trim();
      return a || fallback || "";
    },
    async secret(q) {
      process.stdout.write(`${q}: `);
      muted = true;
      try {
        return await question("");
      } finally {
        muted = false;
      }
    },
    async confirm(q, fallback) {
      const a = (await question(`${q} [${fallback ? "Y/n" : "y/N"}]: `)).trim().toLowerCase();
      return a ? a.startsWith("y") : fallback;
    },
    close() {
      rl?.close();
    },
  };
}

/** For `--yes`: every question takes its default, and one without a default is an error. */
export function defaultsPrompter(): Prompter {
  return {
    async ask(q, fallback) {
      if (fallback === undefined) throw new Error(`${q}: no default; pass it as a flag`);
      return fallback;
    },
    async secret(q) {
      throw new Error(`${q}: pass it as a flag or environment variable with --yes`);
    },
    async confirm(_q, fallback) {
      return fallback;
    },
    close() {},
  };
}
