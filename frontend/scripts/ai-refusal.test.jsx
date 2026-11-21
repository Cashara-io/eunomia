import { describe, expect, test } from "bun:test";
import { AI_ALLOWANCE_USED, AI_DISABLED, aiRefusal } from "../src/lib/aiRefusal";

describe("aiRefusal", () => {
  test("names the used-up allowance and what an owner can do", () => {
    const owner = aiRefusal(AI_ALLOWANCE_USED, true);
    expect(owner?.title).toContain("used its AI allowance");
    expect(owner?.body).toContain("own provider key");
    expect(owner?.body).toContain("renews");
    const member = aiRefusal(AI_ALLOWANCE_USED, false);
    expect(member?.body).toContain("Ask an owner");
  });
  test("says AI is off, and who can switch it on", () => {
    expect(aiRefusal(AI_DISABLED, true)?.body).toContain("Settings > Organisation");
    expect(aiRefusal(AI_DISABLED, false)?.body).toContain("owner or admin");
  });
  test("leaves every other failure to the generic message", () => {
    expect(aiRefusal("internal", true)).toBeNull();
    expect(aiRefusal(undefined, true)).toBeNull();
  });
});
