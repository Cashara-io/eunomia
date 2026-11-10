import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const steps = [12, 13, 14, 16, 20, 24];
const roles = ["caption", "detail", "body", "reading", "heading", "display"];
const globals = readFileSync(new URL("../src/app/globals.css", import.meta.url), "utf8");
const scaleToken = /^var\(--type-(caption|detail|body|reading|heading|display)\)$/;
const cameraToken = /^var\(--(?:name|small)-text,\s*var\(--type-(reading|caption)\)\)$/;

function offScale(source) {
  const failures = [];
  const check = (raw, unitless = false) => {
    const value = raw.trim().replace(/^(["'`])(.*)\1$/, "$2");
    if (scaleToken.test(value) || cameraToken.test(value)) return;
    const literal = value.match(/^(\d+(?:\.\d+)?)(px|rem)?$/);
    if (literal && (literal[2] || unitless)) {
      const size = Number(literal[1]) * (literal[2] === "rem" ? 16 : 1);
      if (steps.includes(size)) return;
    }
    failures.push(raw);
  };
  for (const match of source.matchAll(/\btext-\[([^\]]+)\]/g)) {
    const value = match[1].replace(/^length:/, "");
    if (/^var\(--(?:ink|accent|surface|border|warning)[\w-]*\)$/.test(value)) continue;
    check(value);
  }
  for (const match of source.matchAll(/\bfont-size\s*:\s*([^;\n}]+)/g)) check(match[1]);
  for (const match of source.matchAll(/\bfont\s*:\s*([^;\n}]+)/g)) {
    const value = match[1].trim();
    if (value === "inherit") continue;
    const size = value.match(/^(?:(?:normal|italic|oblique|bold|bolder|lighter|[1-9]00)\s+)*(var\(--[\w-]+\)|[\d.]+(?:px|rem|em|vw|vh|%))(?=\s|\/|$)/);
    if (size) check(size[1]);
    else failures.push(value);
  }
  for (const match of source.matchAll(/(?:["']?\bfontSize["']?\s*:\s*([^,}\n]+)|\b(?:fontSize|font-size)\s*=\s*(?:\{([^}]+)\}|(["'])(.*?)\3))/g)) {
    check(match[1] ?? match[2] ?? match[4], true);
  }
  for (const match of source.matchAll(/\btext-(lg|[3-9]xl|page-title)\b/g)) failures.push(match[0]);
  for (const match of source.matchAll(/<MetricNumber\b[^>]*\bsize=\{([^}]+)\}/g)) check(match[1], true);
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

for (const source of [
  'style={{"fontSize": 15}}',
  '<MetricNumber run={result} size={22} />',
  '<text font-size="11">',
  '--label-size: 11px; font-size: var(--label-size);',
  'font-size: calc(12px + 13px);',
]) test(`typography guard rejects ${source}`, () => {
  expect(offScale(source).length).toBeGreaterThan(0);
});

for (const source of [
  'fontSize: `13px`',
  '<MetricNumber run={result} size={24} />',
  'font: 14px/18px sans-serif;',
]) test(`typography guard accepts ${source}`, () => {
  expect(offScale(source)).toEqual([]);
});
