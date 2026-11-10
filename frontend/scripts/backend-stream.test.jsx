import { afterEach, expect, mock, spyOn, test } from "bun:test";
import { streamBackend } from "../src/lib/backend-stream";

afterEach(() => mock.restore());

async function upload(path) {
  let now = 75_000;
  let controller;
  let tick;
  let upstreamSignal;
  let received = 0;
  let acknowledged = Promise.withResolvers();
  const ready = Promise.withResolvers();
  const timer = 123;
  spyOn(performance, "now").mockImplementation(() => now);
  spyOn(globalThis, "setInterval").mockImplementation((callback) => { tick = callback; return timer; });
  const clear = spyOn(globalThis, "clearInterval").mockImplementation(() => {});
  // Consume the actual transformed stream. An abort must cancel that stream and
  // reach the handler's 408 path, even when no new chunk or EOF arrives.
  spyOn(globalThis, "fetch").mockImplementation(async (_url, init) => {
    if (init.method === "HEAD") {
      return new Response(null, { headers: {
        "x-eunomia-body-limit": String(151 * 1024 * 1024),
        "x-eunomia-upload-allowed": "1",
      } });
    }
    upstreamSignal = init.signal;
    ready.resolve();
    for await (const chunk of init.body) {
      received += chunk.byteLength;
      acknowledged.resolve();
    }
    return Response.json({ bytes: received });
  });
  const body = new ReadableStream({ start(value) { controller = value; } });
  const response = streamBackend(new Request(`http://proxy.test${path}`, {
    method: "POST", body, duplex: "half", headers: { authorization: "Bearer valid" },
  }));
  await ready.promise;
  return {
    response,
    clear,
    timer,
    at(elapsed) { now = 75_000 + elapsed; tick(); },
    aborted() { return upstreamSignal.aborted; },
    async send(bytes) {
      acknowledged = Promise.withResolvers();
      controller.enqueue(new Uint8Array(bytes));
      await acknowledged.promise;
    },
    end() { controller.close(); },
  };
}

for (const path of ["/mcp", "/api/documents?filename=large.csv"]) {
  test(`${path}: honest 128 KiB/s stream succeeds with a two-second pause`, async () => {
    const body = await upload(path);
    const size = 4 * 1024 * 1024;
    for (let sent = 0, elapsed = 0; sent < size; elapsed += 125) {
      body.at(elapsed);
      if (elapsed >= 25_000 && elapsed < 27_000) continue;
      await body.send(16 * 1024);
      sent += 16 * 1024;
    }
    body.end();
    const response = await body.response;
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ bytes: size });
    expect(body.clear).toHaveBeenCalledWith(body.timer);
  });

  for (const rate of [0, 8 * 1024]) {
    test(`${path}: ${rate ? "trickling" : "stalled"} stream returns 408 without EOF`, async () => {
      const body = await upload(path);
      await body.send(2 * 1024 * 1024);
      for (let elapsed = 1000; elapsed < 25_000; elapsed += 1000) {
        body.at(elapsed);
        if (rate) await body.send(rate);
        expect(body.aborted()).toBe(false);
      }
      body.at(25_000);
      const response = await body.response;
      expect(body.aborted()).toBe(true);
      expect(response.status).toBe(408);
      expect(await response.json()).toEqual({ detail: "Upload body timed out" });
      expect(body.clear).toHaveBeenCalledWith(body.timer);
    });
  }
}
