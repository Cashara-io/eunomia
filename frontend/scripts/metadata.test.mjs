import { expect, test } from "bun:test";
import {
  configuredMetadataBase,
  requestMetadataBase,
} from "../src/lib/metadata";

test("private installs tolerate missing and malformed public URLs", () => {
  for (const FRONTEND_URL of [
    undefined,
    "",
    "private-host",
    "/relative",
    "javascript:alert(1)",
    "https://name:password@private/",
  ]) {
    expect(configuredMetadataBase({ FRONTEND_URL })).toBeNull();
  }
  expect(
    configuredMetadataBase({ PUBLIC_URL: "http://eunomia.home:3400/path" })
      ?.href,
  ).toBe("http://eunomia.home:3400/");
  expect(
    configuredMetadataBase({
      FRONTEND_URL: "bad",
      PUBLIC_URL: "https://memory.example.test",
    })?.href,
  ).toBe("https://memory.example.test/");
  expect(
    configuredMetadataBase({
      FRONTEND_URL: "https://primary.test",
      PUBLIC_URL: "https://secondary.test",
    })?.host,
  ).toBe("primary.test");
});

test("request host supplies usable URLs on private installs", () => {
  expect(requestMetadataBase("eunomia.home:3400", "http").href).toBe(
    "http://eunomia.home:3400/",
  );
  expect(requestMetadataBase("memory.home", "https").href).toBe(
    "https://memory.home/",
  );
  for (const host of [null, "bad/host", "bad?query", "name@host", "bad host"])
    expect(requestMetadataBase(host, "https").href).toBe(
      "http://localhost:3000/",
    );
});
