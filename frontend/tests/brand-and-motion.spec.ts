import { test, expect, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import sharp from "sharp";
import type { OntologyOut } from "../src/lib/gen";

const artifactDir = process.env.BRAND_SCREENSHOTS;
const pageErrors = new WeakMap<Page, string[]>();
test.afterEach(({ page }) => {
  expect(pageErrors.get(page) ?? []).toEqual([]);
});
const space = {
  id: "brand-space",
  name: "Design studio",
  kind: "org",
  role: "owner",
  owner_email: "fixture@example.test",
  created_at: null,
};
const settings = {
  theme: {},
  embedding_model: "",
  chat_model: "",
  openai_base_url: "",
  openai_api_key_set: false,
  model_configured: true,
  sync_intervals: {},
  sync_check_connectivity: true,
};
const ontology = (): OntologyOut => ({
  vault: space.id,
  strict: false,
  links: [],
  kinds: Array.from({ length: 19 }, (_, i) => ({
    kind: `studio_${i}`,
    status: "proposed",
    description: "A proposed business kind",
    entities: 0,
    built_in: false,
    properties: [],
    metrics: [],
    samples: [],
    synonyms: [],
  })),
});

async function fixture(page: Page) {
  page.on("pageerror", (error) => {
    const errors = pageErrors.get(page) ?? [];
    errors.push(error.message);
    pageErrors.set(page, errors);
  });
  let data = ontology();
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let json: unknown = [];
    if (path === "/api/auth/me")
      json = {
        id: "fixture",
        email: "fixture@example.test",
        onboarded: true,
        is_admin: true,
        has_password: true,
      };
    else if (path === "/api/auth/bootstrap") json = { has_users: true };
    else if (path === "/api/settings") json = settings;
    else if (path === "/api/update/status")
      json = {
        configured: false,
        current_version: "2.2.0",
        latest_version: "2.2.0",
        update_available: false,
      };
    else if (path === "/api/admin/people")
      json = { results: [], signup: { mode: "closed" } };
    else if (path === "/api/vaults") json = { results: [space] };
    else if (path === "/api/vaults/invitations") json = { results: [] };
    else if (path === "/api/entities/graph") json = { nodes: [], edges: [] };
    else if (path === "/api/entities")
      json = { results: [], total: 0, has_more: false };
    else if (path === "/api/documents")
      json = { results: [], storage: true, max_bytes: 10000000 };
    else if (path.endsWith("/ontology/kinds/decide")) {
      const body: { kind: string; decision: { action: string } } = route
        .request()
        .postDataJSON();
      data = {
        ...data,
        kinds: data.kinds.map((k) =>
          k.kind === body.kind
            ? {
                ...k,
                status:
                  body.decision.action === "approve" ? "approved" : "rejected",
              }
            : k,
        ),
      };
      json = data;
    } else if (path.endsWith("/ontology")) json = data;
    else if (path.endsWith("/readiness")) {
      const counts = { proposed: 19, approved: 0, rejected: 0, observed: 0 };
      json = {
        vault: space.id,
        ready: false,
        strict: false,
        blockers: [],
        kinds: counts,
        properties: counts,
        metrics: counts,
        links: counts,
      };
    }
    await route.fulfill({ json });
  });
  await page.routeWebSocket("**/api/**", (socket) => socket.close());
}

async function screenshot(page: Page, name: string) {
  if (!artifactDir) return;
  await mkdir(artifactDir, { recursive: true });
  await page.screenshot({ path: `${artifactDir}/${name}.png`, fullPage: true });
}

test("brand icon files, manifest and social metadata are served", async ({
  page,
  request,
}) => {
  const response = await request.get("/favicon.ico");
  expect(response.ok()).toBeTruthy();
  const ico = await response.body();
  expect(createHash("sha256").update(ico).digest("hex")).toBe(
    "5c7acc8766b8b28f4bd2b50c026281e27b478fccbaf7989ff29c7e0692bbca7c",
  );
  expect([ico[6], ico[22], ico[38]]).toEqual([16, 32, 48]);
  expect(ico.readUInt16LE(4)).toBe(3);
  expect(ico).toEqual(await readFile("src/app/favicon.ico"));
  const manifestResponse = await request.get("/manifest.webmanifest");
  expect(manifestResponse.ok()).toBeTruthy();
  const manifest = await manifestResponse.json();
  expect(manifest.name).toBe("Eunomia");
  expect(manifest.icons).toEqual([
    { src: "/icon1.png", sizes: "192x192", type: "image/png", purpose: "any" },
    { src: "/icon2.png", sizes: "512x512", type: "image/png", purpose: "any" },
    { src: "/maskable-icon.png", sizes: "512x512", type: "image/png", purpose: "maskable" },
  ]);
  for (const [src, size, hash] of [
    ["/apple-icon.png", 180, "49e805b7a26451abd1f7971ab47ff03d49c13ed8e8ce47595f358fbe57104890"],
    ["/icon1.png", 192, "81fe7747f7ce0cb1f8e42c8545180c59c1293c063271d249fdf6e9f496346b7c"],
    ["/icon2.png", 512, "717283178ecad38420fba5e235a822f114e9ce61d929d2ae44ad9e1daefa0f59"],
    ["/maskable-icon.png", 512, "e0b863ff37cba4b644f6684d05816bc53ff54c7c5955da434d00e0768dbc6ce0"],
  ] satisfies [string, number, string][]) {
    const response = await request.get(src);
    expect(response.ok()).toBeTruthy();
    const bytes = await response.body();
    expect(createHash("sha256").update(bytes).digest("hex")).toBe(hash);
    expect(await sharp(bytes).metadata()).toMatchObject({
      width: size, height: size, hasAlpha: false,
    });
    if (src === "/maskable-icon.png") {
      const pixels = await sharp(bytes).raw().toBuffer();
      let furthest = 0;
      for (let y = 0; y < size; y++)
        for (let x = 0; x < size; x++) {
          const at = (y * size + x) * 3;
          if (pixels[at] !== 250 || pixels[at + 1] !== 250 || pixels[at + 2] !== 247)
            furthest = Math.max(furthest, Math.hypot(x + .5 - size / 2, y + .5 - size / 2));
        }
      expect(furthest).toBeLessThan(size * .4);
    }
  }
  const svg = await (await request.get("/icon.svg")).text();
  expect(createHash("sha256").update(svg).digest("hex")).toBe(
    "572d9ac1a4f0b1b9f562e9bedce0d50c39af1a24417639dcb144b556342ededf",
  );
  expect(svg).toContain("prefers-color-scheme:dark");
  for (const colour of ["#CF412F", "#E8BE39", "#3159B6", "#DD5743", "#ECC64C", "#5A80D8"])
    expect(svg).toContain(colour);
  await page.goto("/login");
  await expect(page.locator('link[rel="manifest"]')).toHaveAttribute(
    "href",
    "/manifest.webmanifest",
  );
  await expect(page.locator('link[rel="apple-touch-icon"]')).toHaveAttribute(
    "href",
    /apple-icon/,
  );
  for (const selector of [
    'meta[property="og:image"]',
    'meta[name="twitter:image"]',
  ]) {
    const value = await page.locator(selector).getAttribute("content");
    const url = new URL(value!);
    expect(["http:", "https:"]).toContain(url.protocol);
    if (process.env.EXPECT_METADATA_ORIGIN)
      expect(url.origin).toBe(process.env.EXPECT_METADATA_ORIGIN);
    expect((await request.get(url.pathname + url.search)).ok()).toBeTruthy();
  }
  await expect(page.locator('meta[property="og:title"]')).toHaveAttribute(
    "content",
    "Eunomia",
  );
  await expect(page.locator('meta[name="twitter:card"]')).toHaveAttribute(
    "content",
    "summary_large_image",
  );
  await screenshot(page, "login");
});

const pendingViews = [
  {
    url: "/",
    api: "/api/auth/tokens",
    label: "Checking your connected apps",
    image: "skeleton-home",
  },
  {
    url: "/entities",
    api: "/api/entities",
    label: "Loading people and topics",
    image: "skeleton-people",
  },
  {
    url: "/vaults",
    api: "/api/vaults",
    label: "Loading memory spaces",
    image: "skeleton-spaces",
  },
  {
    url: "/documents",
    api: "/api/documents",
    label: "Loading documents",
    image: "skeleton-documents",
  },
  {
    url: "/connectors",
    api: "/api/connectors",
    label: "Loading connected sources",
    image: "skeleton-sources",
  },
  ...["", "/schema", "/review"].map((view) => ({
    url: `/ontology${view}?space=${space.id}`,
    api: `/api/vaults/${space.id}/ontology`,
    label: "Loading the ontology",
    image: `skeleton-ontology${view.replace("/", "-")}`,
  })),
  {
    url: "/chat",
    api: "/api/chat/threads",
    label: "Loading conversations",
    image: "skeleton-chat",
  },
  {
    url: "/settings",
    api: "/api/settings",
    label: "Loading settings",
    image: "skeleton-settings",
  },
  {
    url: "/settings?tab=tokens",
    api: "/api/auth/tokens",
    label: "Loading rows",
    image: "skeleton-tokens",
  },
];
for (const view of pendingViews)
  test(`measured skeleton while ${view.url} waits for its API`, async ({
    page,
  }) => {
    await fixture(page);
    let release: () => void = () => {};
    const wait = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route(`**${view.api}*`, async (route) => {
      await wait;
      await route.fallback();
    });
    await page.goto(view.url);
    try {
      await expect(
        page.getByRole("status").filter({ hasText: view.label }).first(),
      ).toBeAttached();
      const busy = page.locator('.loading-skeleton[aria-busy="true"]').first();
      await expect(busy).toBeVisible();
      await expect(busy.locator('[aria-hidden="true"][inert]')).toBeAttached();
      // Wait for real measured bars, rather than merely checking the wrapper.
      await expect(
        busy.locator("[skullet-layer] [skullet-rect]").first(),
      ).toBeAttached();
      await screenshot(page, view.image);
      await expect(busy).toBeVisible();
    } finally {
      release();
    }
    await expect(
      page.getByRole("status").filter({ hasText: view.label }).first(),
    ).not.toBeAttached();
  });

for (const reduced of [false, true])
  test(`review count updates with reduced motion ${reduced}`, async ({
    page,
  }) => {
    await page.emulateMedia({
      reducedMotion: reduced ? "reduce" : "no-preference",
    });
    await fixture(page);
    await page.goto(
      `/ontology/review?space=${space.id}&selected=${encodeURIComponent(JSON.stringify(["kind", "studio_0"]))}`,
    );
    const review = page.getByRole("link", { name: /Review \(\s*19\s*\)/ });
    await expect(review).toBeVisible();
    await expect(review.locator("[data-text-morph]")).toHaveAttribute(
      "data-text-morph",
      reduced ? "off" : "on",
    );
    // Torph loads asynchronously. Wait for its real DOM before changing the value.
    if (!reduced)
      await expect(
        review.locator('[aria-hidden="true"] span').first(),
      ).toBeAttached();
    await screenshot(page, reduced ? "review-reduced" : "count-before");
    const bounds = await page
      .getByRole("navigation", { name: "Ontology views" })
      .boundingBox();
    if (!reduced && artifactDir && bounds)
      await page.screenshot({
        path: `${artifactDir}/count-frame-0.png`,
        clip: bounds,
      });
    await page.getByRole("button", { name: "Approve", exact: true }).click();
    await expect(
      page.getByRole("link", { name: /Review \(\s*18\s*\)/ }),
    ).toBeVisible();
    if (!reduced) {
      await expect
        .poll(
          () =>
            page
              .getByRole("navigation", { name: "Ontology views" })
              .evaluate(
                (node) =>
                  node
                    .getAnimations({ subtree: true })
                    .filter((a) => a.playState === "running").length,
              ),
          { intervals: [10, 20, 50] },
        )
        .toBeGreaterThan(0);
      if (artifactDir && bounds)
        for (const frame of [1, 2, 3])
          await page.screenshot({
            path: `${artifactDir}/count-frame-${frame}.png`,
            clip: bounds,
          });
    }
    if (reduced) {
      expect(
        await page
          .locator("[data-text-morph]")
          .evaluateAll(
            (nodes) =>
              nodes
                .flatMap((n) => n.getAnimations({ subtree: true }))
                .filter((a) => a.playState === "running").length,
          ),
      ).toBe(0);
      await expect(page.locator('[data-text-morph="on"]')).toHaveCount(0);
    }
    await screenshot(page, reduced ? "count-reduced-after" : "count-after");
  });

test("haptic preference defaults on, toggles and survives reload", async ({
  page,
}) => {
  await fixture(page);
  await page.goto("/settings?tab=accessibility");
  const toggle = page.getByRole("switch", { name: "Haptic feedback" });
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await page.reload();
  await expect(toggle).not.toBeChecked();
  expect(
    await page.evaluate(() => localStorage.getItem("eunomia-haptics")),
  ).toBe("off");
  await toggle.check();
  await page.reload();
  await expect(toggle).toBeChecked();
  await screenshot(page, "haptics-setting");
  for (const theme of ["light", "dark"]) {
    await page.evaluate((mode) => {
      document.documentElement.dataset.theme = mode;
    }, theme);
    await page.setViewportSize({ width: 320, height: 760 });
    await expect(toggle).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await screenshot(page, `haptics-${theme}-320`);
  }
});

test("icon proof sheet shows native, 2x and 4x sizes in light and dark chrome", async ({
  page,
  request,
}) => {
  const ico = await (await request.get("/favicon.ico")).body();
  const frames = [16, 32, 48].map((size, i) => {
    const at = 6 + i * 16;
    const png = ico.subarray(
      ico.readUInt32LE(at + 12),
      ico.readUInt32LE(at + 12) + ico.readUInt32LE(at + 8),
    );
    return { label: `ICO ${size}px`, size, src: `data:image/png;base64,${png.toString("base64")}` };
  });
  const svg = await (await request.get("/icon.svg")).body();
  frames.push({ label: "SVG 16px", size: 16, src: `data:image/svg+xml;base64,${svg.toString("base64")}` });
  frames.push({ label: "SVG 32px", size: 32, src: `data:image/svg+xml;base64,${svg.toString("base64")}` });
  frames.push({ label: "SVG 48px", size: 48, src: `data:image/svg+xml;base64,${svg.toString("base64")}` });
  const tabs = ["light", "dark"].map((theme) => `
    <section class="chrome ${theme}"><h2>${theme === "light" ? "Light" : "Dark"} browser chrome</h2>
    ${frames.map(({ label, size, src }) => `<div class="tab-row"><h3>${label}</h3>${[1, 2, 4].map((scale) => `
      <div><p>${scale}x, ${size * scale}px</p><div class="strip"><div class="tab">
      <img alt="${label}, ${theme}, ${scale}x" style="color-scheme:${theme}" width="${size * scale}" height="${size * scale}" src="${src}">
      <span>Eunomia</span><span class="close">×</span></div><span class="plus">+</span></div></div>`).join("")}</div>`).join("")}</section>`).join("");
  const tiles = [];
  for (const [label, path, size, kind] of [
    ["Apple touch", "/apple-icon.png", 180, "ios"],
    ["Manifest any", "/icon1.png", 192, "any"],
    ["Manifest any", "/icon2.png", 512, "any"],
    ["Manifest maskable", "/maskable-icon.png", 512, "maskable"],
  ] satisfies [string, string, number, string][]) {
    const bytes = await (await request.get(path)).body();
    const src = `data:image/png;base64,${bytes.toString("base64")}`;
    tiles.push(`<section class="tile-row"><h2>${label}, ${size}px</h2><div class="tile-scales">${[1, 2, 4].map((scale) => `
      <div><p>${scale}x, ${size * scale}px</p><div class="wallpaper"><div class="tile ${kind}" style="width:${size * scale}px;height:${size * scale}px">
      <img alt="${label} ${size}px, ${scale}x" width="${size * scale}" height="${size * scale}" src="${src}">
      ${kind === "maskable" ? '<div class="safe-zone"></div>' : ""}</div>
      ${kind === "ios" ? '<p class="app-name">Eunomia</p>' : ""}</div>
      ${kind === "maskable" && scale === 1 ? `<p>Circle mask at native size</p><img class="circle-mask" alt="Maskable icon with circular crop" width="${size}" height="${size}" src="${src}">` : ""}
      </div>`).join("")}</div></section>`);
  }
  const html = `<!doctype html><html><head><meta charset="utf-8"><style>
    *{box-sizing:border-box}body{margin:0;padding:40px;width:4000px;background:#f0f0eb;color:#171717;font:22px system-ui}
    h1{font-size:44px;margin:0 0 12px}h2{font-size:30px;margin:0 0 24px}h3{font-size:20px;font-weight:600}p{margin:12px 0 16px}
    img{display:block;image-rendering:pixelated}img[src^="data:image/svg"]{image-rendering:auto}
    .intro{margin-bottom:36px}.themes{display:grid;grid-template-columns:1fr 1fr;gap:32px}.chrome{padding:28px;border-radius:20px}
    .light{background:#fff;color:#171717;color-scheme:light}.dark{background:#202124;color:#eee;color-scheme:dark}
    .tab-row{display:grid;grid-template-columns:130px repeat(3,1fr);gap:24px;align-items:center;margin:12px 0 28px}
    .strip{background:#dee1e6;display:flex;align-items:center;padding:12px 12px 0;min-height:70px}.dark .strip{background:#303134}
    .tab{display:flex;align-items:center;gap:18px;background:#fff;border-radius:14px 14px 0 0;padding:14px;flex:1}
    .dark .tab{background:#202124}.close{margin-left:auto}.plus{margin:0 16px}
    .tile-row{margin-top:36px;background:#fff;padding:28px;border-radius:20px}.tile-scales{display:grid;grid-template-columns:592px 1104px 2128px;gap:20px}
    .wallpaper{background:linear-gradient(135deg,#546b71,#242a40 75%);padding:24px;width:max-content}
    .tile{position:relative;overflow:hidden}.ios{border-radius:22.37%;box-shadow:0 3px 10px #0004}.app-name{color:#fff;text-align:center}
    .safe-zone{position:absolute;width:80%;height:80%;top:10%;left:10%;border:3px dashed #17171788;border-radius:50%}
    .circle-mask{border-radius:50%}
  </style></head><body><h1>Eunomia square icons, v2</h1><p class="intro">Native size, 2x and 4x. Raster enlargements use nearest-neighbour pixels. SVG responds to each tab strip's colour scheme. Home tiles use opaque off-white artwork. Dashed circle is the maskable 80% safe zone.</p>
  <div class="themes">${tabs}</div>${tiles.join("")}</body></html>`;
  await page.setViewportSize({ width: 4000, height: 1000 });
  await page.setContent(html);
  await page.locator("img").evaluateAll(async (images) => {
    await Promise.all(images.map((img) => img instanceof HTMLImageElement ? img.decode() : Promise.resolve()));
  });
  if (artifactDir) {
    await mkdir(artifactDir, { recursive: true });
    await writeFile(`${artifactDir}/icons-v2.html`, html);
  }
  await screenshot(page, "icons-v2");
});

for (const mode of ["enabled", "disabled", "reduced", "desktop"])
  test(`haptics ${mode} gates a successful review action`, async ({
    browser,
  }) => {
    const context = await browser.newContext({
      hasTouch: mode !== "desktop",
      isMobile: mode !== "desktop",
      viewport: { width: 390, height: 844 },
      reducedMotion: mode === "reduced" ? "reduce" : "no-preference",
    });
    try {
      const page = await context.newPage();
      await page.addInitScript((off) => {
        if (off) localStorage.setItem("eunomia-haptics", "off");
        Object.defineProperty(navigator, "vibrate", {
          configurable: true,
          value: () => {
            document.documentElement.dataset.vibrations = String(
              Number(document.documentElement.dataset.vibrations ?? 0) + 1,
            );
            return true;
          },
        });
      }, mode === "disabled");
      await fixture(page);
      await page.goto(
        `/ontology/review?space=${space.id}&selected=${encodeURIComponent(JSON.stringify(["kind", "studio_0"]))}`,
      );
      await page.getByRole("button", { name: "Approve", exact: true }).click();
      await expect(
        page
          .locator("[data-text-morph] > .sr-only")
          .filter({ hasText: /^18$/ })
          .last(),
      ).toBeAttached();
      if (mode === "enabled")
        await expect(page.locator("html")).toHaveAttribute(
          "data-vibrations",
          /[1-9]/,
        );
      else
        expect(
          await page.locator("html").getAttribute("data-vibrations"),
        ).toBeNull();
    } finally {
      await context.close();
    }
  });

async function generationFixture(page: Page) {
  await fixture(page);
  let finish = () => {};
  await page.route("**/api/chat/threads**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (route.request().method() === "GET") return route.fulfill({ json: [] });
    if (path === "/api/chat/threads")
      return route.fulfill({
        json: { id: "generation", title: "Generation fixture" },
      });
    await new Promise<void>((resolve) => {
      finish = resolve;
    });
    await route
      .fulfill({
        contentType: "text/event-stream",
        body: 'data: {"type":"text","delta":"The generated reply is ready."}\n\n',
      })
      .catch(() => {});
  });
  return { finish: () => finish() };
}

async function trackWebGL(
  page: Page,
  mode: "normal" | "disabled" | "failed" = "normal",
) {
  await page.addInitScript((mode) => {
    const nativeGetContext = HTMLCanvasElement.prototype.getContext;
    const live = new Set<WebGLRenderingContext>();
    const known = new WeakSet<WebGLRenderingContext>();
    const extensions = new WeakSet<object>();
    let attempts = 0,
      closed = 0;
    const report = () => {
      document.documentElement.dataset.webglLive = String(live.size);
      document.documentElement.dataset.webglClosed = String(closed);
      document.documentElement.dataset.webglAttempts = String(attempts);
    };
    Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
      configurable: true,
      value: function (...args: unknown[]) {
        if (args[0] === "webgl") {
          attempts++;
          report();
          if (mode === "disabled") return null;
        }
        const result = Reflect.apply(nativeGetContext, this, args);
        if (result instanceof WebGLRenderingContext && !known.has(result)) {
          known.add(result);
          live.add(result);
          report();
          if (mode === "failed") result.createShader = () => null;
          const nativeGetExtension = result.getExtension.bind(result);
          result.getExtension = (name: string) => {
            const extension = nativeGetExtension(name);
            if (
              name === "WEBGL_lose_context" &&
              extension &&
              !extensions.has(extension)
            ) {
              extensions.add(extension);
              const lose = extension.loseContext.bind(extension);
              extension.loseContext = () => {
                if (live.delete(result)) closed++;
                report();
                lose();
              };
            }
            return extension;
          };
        }
        return result;
      },
    });
  }, mode);
}

for (const mode of [
  "disabled",
  "failed",
  "reduced",
  "save-data",
  "low-battery",
] as const)
  test(`chat generation fallback: ${mode}`, async ({ page }) => {
    await trackWebGL(
      page,
      mode === "disabled" || mode === "failed" ? mode : "normal",
    );
    if (mode === "reduced")
      await page.emulateMedia({ reducedMotion: "reduce" });
    if (mode === "save-data")
      await page.addInitScript(() =>
        Object.defineProperty(navigator, "connection", {
          value: { saveData: true },
          configurable: true,
        }),
      );
    if (mode === "low-battery")
      await page.addInitScript(() =>
        Object.defineProperty(navigator, "getBattery", {
          value: async () => ({ charging: false, level: 0.1 }),
          configurable: true,
        }),
      );
    const stream = await generationFixture(page);
    await page.goto("/chat");
    await page
      .getByRole("textbox", { name: "Message" })
      .fill("Write a short reply");
    const box = page.locator("[data-generation-box]").last();
    const before = await box.boundingBox();
    await page.getByRole("button", { name: "Send", exact: true }).click();
    try {
      await expect(box).toHaveAttribute("aria-busy", "true");
      await expect(
        page.getByRole("status").filter({ hasText: /^Generating$/ }),
      ).toBeAttached();
      if (mode === "disabled")
        await expect(page.locator("html")).toHaveAttribute(
          "data-webgl-attempts",
          /[1-9]/,
        );
      if (mode === "failed")
        await expect(page.locator("html")).toHaveAttribute(
          "data-webgl-closed",
          "1",
        );
      await expect(box.locator("[data-generation-effect]")).toHaveAttribute(
        "data-mode",
        mode === "reduced" ? "still" : "fallback",
      );
      await expect(box.locator(".generation-fallback")).toBeVisible();
      const after = await box.boundingBox();
      expect(after?.height).toBe(before?.height);
      expect(after?.width).toBe(before?.width);
      if (mode === "reduced")
        expect(
          await box
            .locator(".generation-fallback")
            .evaluate(
              (node) => getComputedStyle(node, "::after").animationName,
            ),
        ).toBe("none");
      await screenshot(page, `glimm-fallback-${mode}`);
    } finally {
      stream.finish();
    }
    await expect(box).toHaveAttribute("aria-busy", "false");
    await expect(
      page.getByText("The generated reply is ready.", { exact: true }),
    ).toBeVisible();
    expect(
      (await page.locator("html").getAttribute("data-webgl-live")) ?? "0",
    ).toBe("0");
  });

test("glimm releases its context across six sequential chat generations", async ({
  page,
}) => {
  await trackWebGL(page);
  const coreChunks: { url: string; bytes: number; gzip: number }[] = [];
  page.on("response", (response) => {
    if (response.request().resourceType() !== "script") return;
    void response
      .text()
      .then((source) => {
        if (source.includes("[glimm] shader compile error"))
          coreChunks.push({
            url: response.url(),
            bytes: Buffer.byteLength(source),
            gzip: gzipSync(Buffer.from(source)).length,
          });
      })
      .catch(() => {});
  });
  const stream = await generationFixture(page);
  await page.goto("/chat");
  await expect(page.getByRole("textbox", { name: "Message" })).toBeVisible();
  expect(coreChunks).toHaveLength(0);
  for (let i = 0; i < 6; i++) {
    await page.getByRole("textbox", { name: "Message" }).fill(`Reply ${i + 1}`);
    await page.getByRole("button", { name: "Send", exact: true }).click();
    try {
      const effect = page.locator(
        '[data-generation-effect][data-mode="webgl"]',
      );
      await expect(effect).toBeVisible();
      await expect.poll(() => coreChunks.length).toBe(1);
      if (artifactDir)
        await writeFile(
          `${artifactDir}/glimm-lazy-chunk.json`,
          JSON.stringify(coreChunks, null, 2),
        );
      await expect(page.locator("html")).toHaveAttribute(
        "data-webgl-live",
        "1",
      );
      if (i === 0 && artifactDir) {
        const box = await effect.boundingBox();
        if (box)
          for (let frame = 0; frame < 4; frame++) {
            await page.screenshot({
              path: `${artifactDir}/glimm-frame-${frame}.png`,
              clip: box,
            });
            // Sample the moving band at distinct positions in its 1.8 second sweep.
            await page.waitForTimeout(320);
          }
      }
    } finally {
      stream.finish();
    }
    await expect(page.locator("[data-generation-box]").last()).toHaveAttribute(
      "aria-busy",
      "false",
    );
    await expect(page.locator("html")).toHaveAttribute("data-webgl-live", "0");
    await expect(page.locator("html")).toHaveAttribute(
      "data-webgl-closed",
      String(i + 1),
    );
  }
});

test("a pending generated block gets a bounded sweep and resolves without changing its reserved size", async ({
  page,
}) => {
  await trackWebGL(page);
  await fixture(page);
  const view = {
    view: "ui",
    text: "Resolved generated block",
    spec: {
      root: "copy",
      elements: {
        copy: { type: "Text", props: { text: "Resolved generated block" } },
      },
    },
    data: { sources: {} },
  };
  await page.route("**/api/chat/threads", (route) =>
    route.fulfill({ json: [{ id: "block", title: "Generated block" }] }),
  );
  await page.route("**/api/chat/threads/block/history", (route) =>
    route.fulfill({
      json: [
        {
          role: "assistant",
          content: "",
          tool_calls: [
            {
              id: "ui",
              type: "function",
              function: { name: "show_view", arguments: "{}" },
            },
          ],
        },
        { role: "tool", content: JSON.stringify(view) },
      ],
    }),
  );
  let release = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/_next/static/chunks/*.js", async (route) => {
    const response = await route.fetch();
    const source = await response.text();
    if (source.includes('"generative-ui"') && source.includes("65536"))
      await held;
    await route.fulfill({ response });
  });
  await page.goto("/chat");
  const placeholder = page.locator(".generation-placeholder");
  try {
    await expect(placeholder).toBeVisible();
    await expect(
      placeholder.locator("[data-generation-effect]"),
    ).toHaveAttribute("data-mode", "webgl");
    expect((await placeholder.boundingBox())?.height).toBe(240);
    await screenshot(page, "glimm-generated-block");
  } finally {
    release();
  }
  await expect(
    page
      .locator(".generative-ui p")
      .filter({ hasText: "Resolved generated block" }),
  ).toBeVisible();
  await expect(placeholder).toHaveCount(0);
  expect(
    await page
      .locator(".generative-ui")
      .evaluate((node) => node.parentElement?.getBoundingClientRect().height),
  ).toBe(240);
  await expect(page.locator("html")).toHaveAttribute("data-webgl-live", "0");
});

for (const width of [1280, 390])
  test(`person details has a positioned skeleton at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 800 });
    await fixture(page);
    const person = {
      id: "person-alpha",
      name: "Alex",
      kind: "person",
      aliases: [],
      summary: null,
      owner_email: "fixture@example.test",
      memory: [],
      relations: [],
      properties: [],
    };
    await page.route("**/api/entities?*", (route) =>
      route.fulfill({ json: { results: [person], total: 1, has_more: false } }),
    );
    let release = () => {};
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route("**/api/entities/person-alpha", async (route) => {
      await pending;
      await route.fulfill({ json: person });
    });
    await page.goto("/entities");
    await page.getByRole("row").filter({ hasText: "Alex" }).click();
    const panel = page.getByRole("complementary", {
      name: "Loading person or topic",
    });
    try {
      await expect(panel).toBeVisible();
      expect(
        await panel.evaluate((node) => getComputedStyle(node).position),
      ).toBe(width < 768 ? "fixed" : "absolute");
      await expect(
        panel.locator("[skullet-layer] [skullet-rect]").first(),
      ).toBeAttached();
      await screenshot(page, `skeleton-person-${width}`);
    } finally {
      release();
    }
    await expect(panel).toHaveCount(0);
    await expect(
      page.getByRole("complementary", { name: "Alex details" }),
    ).toBeVisible();
  });
