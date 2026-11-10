import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const steps = [12, 13, 14, 16, 20, 24];
const roles = ["caption", "detail", "body", "reading", "heading", "display"];
const globals = readFileSync(new URL("../src/app/globals.css", import.meta.url), "utf8");
const tokenPattern = /var\(--type-([\w-]+)\)/g;

function offScale(source) {
  const failures = [];
  const check = (value) => {
    for (const match of value.matchAll(/(-?\d*\.?\d+)(px|rem|em|vw|vh|%)/g)) {
      const size = Number(match[1]) * (match[2] === "rem" ? 16 : 1);
      if (!["px", "rem"].includes(match[2]) || !steps.includes(size)) failures.push(match[0]);
    }
    for (const match of value.matchAll(tokenPattern)) {
      if (!roles.includes(match[1])) failures.push(match[0]);
    }
  };
  for (const match of source.matchAll(/\btext-\[([^\]]+)\]/g)) check(match[1]);
  for (const match of source.matchAll(/\b(?:font-size|font)\s*:\s*([^;\n}]+)/g)) check(match[1]);
  for (const match of source.matchAll(/\bfontSize\s*(?:=\s*\{([^}]+)\}|=\s*["']([^"']+)["']|:\s*([^,}\n]+))/g)) {
    const value = (match[1] ?? match[2] ?? match[3]).trim();
    check(value);
    if (/^["']?\d+(?:\.\d+)?["']?$/.test(value) && !steps.includes(Number(value.replace(/["']/g, "")))) failures.push(value);
    if (/^\d+(?:\.\d+)?$/.test(value)) continue;
    if (!/["']|var\(/.test(value)) failures.push(value);
  }
  for (const match of source.matchAll(/\btext-(lg|[3-9]xl|page-title)\b/g)) failures.push(match[0]);
  return failures;
}

function files(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? files(path) : [path];
  });
}

test("Ontology uses the six shared typography tokens", () => {
  for (const [index, role] of roles.entries()) {
    const value = globals.match(new RegExp(`--type-${role}:\\s*([\\d.]+)rem;`));
    expect(Number(value?.[1]) * 16).toBe(steps[index]);
  }
  for (const directory of ["../src/components/twin", "../src/app/(app)/ontology"]) {
    for (const path of files(new URL(directory, import.meta.url).pathname)) {
      expect(offScale(readFileSync(path, "utf8")), path).toEqual([]);
    }
  }
});

test("typography guard catches off-scale utilities, CSS and SVG or style literals", () => {
  for (const source of [
    'className="text-[12.5px]"', 'className="md:text-[length:11.5px]"',
    'className="text-lg"', 'className="text-3xl"',
    "font-size: 1.125rem;", "font: 500 15px/1.5 sans-serif;",
    "font-size: clamp(12px, 2vw, 30px);", "font-size: var(--type-page-title);",
    'fontSize="17"', "fontSize={15}", "fontSize: 15", "fontSize: '11px'", 'fontSize={"13.5px"}',
  ]) expect(offScale(source).length, source).toBeGreaterThan(0);
  for (const step of steps) {
    expect(offScale(`className="text-[${step}px]" fontSize={${step}}`)).toEqual([]);
    expect(offScale(`fontSize="${step}"; font-size: ${step / 16}rem;`)).toEqual([]);
  }
  expect(offScale('className="text-[var(--ink-dim)]" font-size: var(--type-detail);')).toEqual([]);
});
