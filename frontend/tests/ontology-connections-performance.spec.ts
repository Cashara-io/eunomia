import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { mockConnections, neighbourhood } from "./fixtures/connections";

// Keep Playwright snapshot capture out of the timings. The spec saves its own CDP trace.
test.use({ trace: "off" });
test.skip(process.env.CONNECTIONS_PERF !== "1", "Set CONNECTIONS_PERF=1 for local performance measurements");

// Deliberately exceeds the service contract. This measures the renderer only.
test("synthetic scene and supported scene performance traces", async ({
  page,
  browserName,
}) => {
  test.setTimeout(300000);
  page.setDefaultTimeout(10_000);
  await page.setViewportSize({ width: 1440, height: 900 });
  const dir = process.env.ONTOLOGY_SCREENSHOTS ?? test.info().outputDir;
  await mkdir(dir, { recursive: true });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("HeapProfiler.enable");
  await cdp.send("HeapProfiler.collectGarbage");

  const data = neighbourhood(999, 3000);
  await mockConnections(page, "populated", data);
  await page.routeWebSocket("**/api/events/ws*", ws =>
    ws.send(JSON.stringify({ type: "hello", vaults: { business: 0 } })),
  );
  await page.goto("/ontology/connections?space=business");
  await expect(
    page.getByRole("heading", { name: "Start with something you know." }),
  ).toBeVisible();
  await cdp.send("HeapProfiler.collectGarbage");
  const baseline = await cdp.send("Runtime.getHeapUsage");
  await cdp.send("Tracing.start", {
    categories: "devtools.timeline,blink.user_timing",
    transferMode: "ReturnAsStream",
  });
  const initial = Date.now();
  await page.goto("/ontology/connections?space=business&focus=house");
  const group = page.locator("[data-group]");
  await expect(group).toContainText("999 products");
  const initialMs = Date.now() - initial;
  const stressLayoutMs = Number(
    await page.locator("[data-layout-ms]").getAttribute("data-layout-ms"),
  );
  await group.click();
  await expect(group).toHaveAttribute("aria-expanded", "true");
  await cdp.send("HeapProfiler.collectGarbage");
  const loaded = await cdp.send("Runtime.getHeapUsage");
  const frames = await page.locator("[data-camera]").evaluate(async (el) => {
    const values: number[] = [];
    const start = performance.now();
    let last = start;
    await new Promise<void>((resolve) => {
      function frame(now: number) {
        values.push(now - last);
        last = now;
        el.setAttribute(
          "style",
          `transform:translate(${Math.sin((now - start) / 1000) * 200 - 200}px,${Math.cos((now - start) / 1000) * 100 - 80}px)`,
        );
        if (now - start < 10000) requestAnimationFrame(frame);
        else resolve();
      }
      requestAnimationFrame(frame);
    });
    return values.slice(1);
  });
  const complete = new Promise<string>((resolve) =>
    cdp.once("Tracing.tracingComplete", (event) => resolve(event.stream ?? "")),
  );
  await cdp.send("Tracing.end");
  const stream = await complete;
  let trace = "";
  for (;;) {
    const chunk = await cdp.send("IO.read", { handle: stream });
    trace += chunk.data;
    if (chunk.eof) break;
  }
  await cdp.send("IO.close", { handle: stream });
  await writeFile(`${dir}/canvas-trace.json`, trace);
  const stats = (values: number[]) => {
    const sorted = [...values].sort((a, b) => a - b);
    return {
      n: values.length,
      p95: sorted[Math.floor(sorted.length * 0.95)],
      worst: sorted.at(-1),
      over20ms: values.filter((n) => n > 20).length,
    };
  };
  const incrementalMB = (loaded.usedSize - baseline.usedSize) / 1024 / 1024;
  expect(incrementalMB).toBeLessThan(150);
  await page.unroute("**/api/tools/**");
  await page.route("**/api/tools/**", (r) =>
    r.fulfill({ json: neighbourhood(149, 600) }),
  );
  async function measuredClick(button: ReturnType<typeof page.getByRole>) {
    await page.evaluate(() => {
      document.addEventListener(
        "click",
        () => {
          performance.mark("cold-input");
          requestAnimationFrame(() =>
            requestAnimationFrame(() => performance.mark("cold-painted")),
          );
        },
        { once: true, capture: true },
      );
    });
    await button.click();
    await page.waitForFunction(
      () => performance.getEntriesByName("cold-painted").length > 0,
    );
    return page.evaluate(() => {
      const start =
        performance.getEntriesByName("cold-input").at(-1)?.startTime ?? 0;
      const end =
        performance.getEntriesByName("cold-painted").at(-1)?.startTime ?? 0;
      performance.clearMarks("cold-input");
      performance.clearMarks("cold-painted");
      return end - start;
    });
  }
  const coldInputMs: number[] = [];
  const layoutMs: number[] = [];
  async function cold() {
    const t = Date.now();
    await page.reload();
    await expect(page.locator("[data-group]")).toBeVisible();
    const contentMs = Date.now() - t;
    layoutMs.push(
      Number(
        await page.locator("[data-layout-ms]").getAttribute("data-layout-ms"),
      ),
    );
    coldInputMs.push(
      await measuredClick(page.locator('[data-object="house"]').first()),
    );
    return contentMs;
  }
  const coldMs: number[] = [];
  for (let i = 0; i < 30; i++) coldMs.push(await cold());
  const input = async (n: number) => {
    const samples: number[] = [];
    for (let i = 0; i < n; i++) {
      await page.evaluate(() => {
        document.addEventListener(
          "click",
          () => {
            performance.mark("connection-input-start");
            requestAnimationFrame(() =>
              requestAnimationFrame(() =>
                performance.mark("connection-input-painted"),
              ),
            );
          },
          { once: true, capture: true },
        );
      });
      const button =
        i % 3 === 0
          ? page.getByRole("button", { name: "Show as table" })
          : i % 3 === 1
            ? page
                .getByRole("table")
                .first()
                .getByRole("button", { name: "House blend 1kg" })
            : page.getByRole("button", { name: "Show canvas" });
      await button.click();
      await page.waitForFunction(
        () =>
          performance.getEntriesByName("connection-input-painted").length > 0,
      );
      samples.push(
        await page.evaluate(() => {
          const start =
            performance.getEntriesByName("connection-input-start").at(-1)
              ?.startTime ?? 0;
          const end =
            performance.getEntriesByName("connection-input-painted").at(-1)
              ?.startTime ?? 0;
          performance.clearMarks("connection-input-start");
          performance.clearMarks("connection-input-painted");
          return end - start;
        }),
      );
    }
    return samples;
  };
  const warm = await input(102);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  const slow = await input(102);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  const closeScene = async () => {
    await page.evaluate(() =>
      window.history.pushState(
        null,
        "",
        "/ontology/connections?space=business",
      ),
    );
    await expect(
      page.getByRole("heading", { name: "Start with something you know." }),
    ).toBeVisible();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    await cdp.send("HeapProfiler.collectGarbage");
    return (await cdp.send("Runtime.getHeapUsage")).usedSize;
  };
  const returnedSize = await closeScene();
  const closedHeaps: number[] = [];
  for (let i = 0; i < 3; i++) {
    await page.evaluate(() =>
      window.history.pushState(
        null,
        "",
        "/ontology/connections?space=business&focus=house",
      ),
    );
    await expect(page.locator("[data-group]")).toBeVisible();
    closedHeaps.push(await closeScene());
  }
  const report = {
    browserName,
    browser: page.context().browser()?.version(),
    platform: process.platform,
    arch: process.arch,
    scene: {
      nodes: 1000,
      edges: 3000,
      seconds: 10,
      claim:
        "Synthetic renderer stress only. Service remains limited to 150 nodes and 600 edges.",
      initialMs,
      workerLayoutMs: stressLayoutMs,
      frames: stats(frames),
      incrementalMB,
    },
    supported: {
      nodes: 150,
      edges: 600,
      cold30: stats(coldMs),
      coldInput30: stats(coldInputMs),
      workerLayout30: stats(layoutMs),
      warm102: stats(warm),
      cpu4x102: stats(slow),
    },
    timingMethod:
      "Trusted click capture to second requestAnimationFrame, an upper bound for next paint. Synthetic pan uses CSS transforms with viewport culling.",
    repeatedClosedHeapMB: closedHeaps.map((n) => n / 1024 / 1024),
    closedHeapGrowthPercent:
      (Math.max(...closedHeaps) / returnedSize - 1) * 100,
    returnedHeapMB: returnedSize / 1024 / 1024,
  };
  await writeFile(`${dir}/performance.json`, JSON.stringify(report, null, 2));
  expect(report.supported.coldInput30.p95).toBeLessThan(100);
  expect(report.closedHeapGrowthPercent).toBeLessThan(10);
  expect(report.supported.warm102.p95).toBeLessThan(100);
  expect(report.supported.cpu4x102.p95).toBeLessThan(100);
});
