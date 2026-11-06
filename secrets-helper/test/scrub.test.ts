import { test } from "node:test";
import assert from "node:assert/strict";
import { Scrubber } from "../src/scrub.js";

test("tokens and values are replaced in every form", () => {
  const s = new Scrubber().token("tok+en/with=chars").value("the-value");
  const text = `a tok+en/with=chars b ${encodeURIComponent("tok+en/with=chars")} c ${Buffer.from("tok+en/with=chars").toString("base64url")} d the-value`;
  assert.equal(s.clean(text), "a [token] b [token] c [token] d [value]");
});
