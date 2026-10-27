import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { parseToml, stringifyToml } from "../src/project.js";
import { parseFrontmatter, validName } from "../src/skills.js";

describe("toml", () => {
  it("round-trips strings with quotes, backslashes and control characters", () => {
    const data = { server: { url: "http://x:1" }, vault: { id: "vault:abc", name: 'Ada "A" \\ B\n\tc\u0001' } };
    const text = stringifyToml(data, ["a comment"]);
    assert.match(text, /^# a comment\n\n\[server\]\nurl = "http:\/\/x:1"\n/);
    assert.deepEqual(parseToml(text), data);
  });

  it("reads comments, blank lines and trailing comments", () => {
    assert.deepEqual(parseToml('# hi\n\n[a] # x\nk = "v" # y\n  j="w"\n'), { a: { k: "v", j: "w" } });
    assert.deepEqual(parseToml('[a]\nk = "\\u00e9"\n'), { a: { k: "é" } });
  });

  it("refuses what it does not understand", () => {
    for (const [bad, why] of [
      ['k = "v"', /outside a \[section\]/],
      ["[a]\nk = 1", /expected/],
      ["[a]\nk = 'v'", /expected/],
      ['[a]\nk = "v', /expected/],
      ['[a]\nk = "a\\qb"', /unsupported escape/],
      ['[a]\nk = "v"\nk = "w"', /appears twice/],
      ["[a]\n[a]", /appears twice/],
      ["[a.b]", /expected/],
    ] as const) {
      assert.throws(() => parseToml(bad), why, bad);
    }
  });
});

describe("frontmatter", () => {
  it("reads name, description and body", () => {
    const r = parseFrontmatter("---\nname: deploy\ndescription: When it breaks. # note\n---\n\n# Body\n\ntext\n\n");
    assert.deepEqual(r, { fields: { name: "deploy", description: "When it breaks." }, body: "# Body\n\ntext" });
  });

  it("handles quotes, block scalars, folded lines and nested keys it skips", () => {
    const r = parseFrontmatter(
      [
        "---",
        "name: 'it''s'",
        'title: "a: \\"b\\""',
        "description: >",
        "  one",
        "  two",
        "",
        "  three",
        "notes: |",
        "  line 1",
        "  line 2",
        "metadata:",
        "  owner: ada",
        "  tags: [a, b]",
        "summary: first",
        "  second",
        "---",
        "body",
      ].join("\n"),
    );
    assert.deepEqual(r.fields, { name: "it's", title: 'a: "b"', description: "one two three", notes: "line 1\nline 2", summary: "first second" });
    assert.equal(r.body, "body");
  });

  it("allows a comment after a quoted value, and reads null or ~ as missing", () => {
    const r = parseFrontmatter(`---\nname: "deploy" # the id\ntitle: 'it''s' # x\ndescription: ~\nlicense: null\n---\nbody\n`);
    assert.deepEqual(r.fields, { name: "deploy", title: "it's", description: "", license: "" });
    assert.throws(() => parseFrontmatter('---\nname: "a" b\n---\n'), /unterminated/);
  });

  it("accepts CRLF and a byte order mark", () => {
    assert.equal(parseFrontmatter("\uFEFF---\r\nname: a\r\n---\r\nbody\r\n").fields.name, "a");
  });

  it("refuses broken frontmatter", () => {
    assert.throws(() => parseFrontmatter("no frontmatter"), /must start with/);
    assert.throws(() => parseFrontmatter("---\nname: a\n"), /no closing/);
    assert.throws(() => parseFrontmatter("---\njust words\n---\n"), /expected "key: value"/);
    assert.throws(() => parseFrontmatter("---\nname: a\nname: b\n---\n"), /appears twice/);
    assert.throws(() => parseFrontmatter('---\nname: "a\n---\n'), /unterminated/);
    assert.throws(() => parseFrontmatter('---\nname: "a"\n  more\n---\n'), /one line/);
  });
});

describe("skill names", () => {
  it("follow the server's rule", () => {
    for (const ok of ["a", "0", "deploy-rollback", "a-", "a".repeat(64)]) assert.ok(validName(ok), ok);
    for (const bad of ["", "-a", "A", "has space", "under_score", "ünï", "a".repeat(65)]) assert.ok(!validName(bad), bad);
  });
});
