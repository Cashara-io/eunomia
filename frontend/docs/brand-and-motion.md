# Brand icons and motion

The sidebar mark in `EunomiaMark.tsx` is the source for the red circle, yellow square and blue triangle. Square icons arrange the red circle and yellow square above a centred blue triangle. Their 16-unit geometry uses whole-pixel bounds at 16px and scales directly to the 32px and 48px ICO frames. The assets use the exact flat fills and off-white canvas from DESIGN.md. The SVG swaps to the documented dark colours and background with prefers-color-scheme. Apple and manifest PNGs have opaque backgrounds without baked-in rounded corners. A separate 512px maskable icon scales the mark to .62 around its centre, keeping every shape inside the circular 80% safe zone. `scripts/brand-assets.mjs` regenerates the ICO, theme-aware SVG, Apple icon, 192/512 manifest icons, the separate maskable icon and the two 1200 by 630 social images. Geist is already bundled under its open font licence. The artwork is original Eunomia geometry. No remote font or image service is needed. Run `bun scripts/brand-assets.mjs --icons-only` to preserve the existing social images.

Next file conventions provide the icon links, manifest and social image metadata. Public URL priority is FRONTEND_URL, then PUBLIC_URL. Invalid or non-web URLs are ignored. Without a configured public URL, metadata uses the request host and forwarded HTTPS scheme. This makes those pages render per request, so private hosts do not inherit a localhost preview URL. Configured URLs can still be prerendered. Public URLs should be origins, with any path discarded.

PR #201 was open at implementation time and main used Next 16.3.4. Images are static PNGs and do not import next/og. The unreferenced Vercel and Next logos were removed. Login, consent and onboarding already used the correct mark.

## Libraries

Exact versions: skullet 0.0.3, torph 0.1.3, web-haptics 0.0.6 and glimm 0.4.1, all MIT. Their published React entry points and installed type declarations were checked against their sites and GitHub READMEs:

- [Skullet site](https://skullet.lochie.me), [React, SSR, accessibility and theming](https://github.com/lochie/skullet/blob/main/packages/skullet/README.md).
- [Torph site](https://torph.lochie.me), [React and reduced motion](https://github.com/lochie/torph/blob/main/packages/torph/README.md).
- [Glimm demo and API](https://glimm.dev), [pinned npm package and README](https://www.npmjs.com/package/glimm/v/0.4.1).
- [Web Haptics site](https://haptics.lochie.me), [API and support](https://github.com/lochie/web-haptics/blob/main/packages/web-haptics/README.md).

No React 19 or Next 16 incompatibility was found. Glimm 0.4.1 deletes its programs, buffers and shaders in destroy(), but does not explicitly lose its browser context. The app wrapper calls WEBGL_lose_context after destroy(), including when initialisation fails. No library substitution was needed. The skullet, torph and glimm engines are lazy chunks. A CSS sample keeps the layout while skullet loads. Haptics loads only for an allowed action.

`LoadingSkeleton` measures representative layouts with skullet. It covers Home, People and topics, the entity inspector, memory spaces and members, Documents, Connected sources and extension/password-manager/request panels, Ontology Overview/Structure/Review/kind/readiness, chat conversations/history, and Settings loading forms and tables. Samples contain no real user data or enabled controls. Skeleton content is inert and hidden from screen readers. A polite status sits outside the aria-busy region. Responsive document columns stay aligned. Static tabs need no loading state.

`MorphText` applies torph to short statuses, sync button words, update status, Home space/entity/memory counts, entity memory/relation counts, Review counts and chat step/thinking labels. Values over 48 characters stay static. `MorphCount` passes the app's formatNumber result as a string. Prose is never passed to numeric formatting. Reduced motion uses plain text and does not mount torph. Screen readers receive the current text once.

The haptic wrapper gates the library on a coarse pointer, Vibration API support, the saved browser preference and reduced motion. The switch is under Settings, Accessibility and defaults on. Feedback follows a successful review decision, settings or memory save, successful copy, chat send or an error. Desktop and unsupported browsers are silent. Library debug audio and switch injection stay off. The library does not offer a reduced-motion option, so the wrapper enforces it. Phone behaviour was checked with an instrumented Vibration API in Chrome mobile emulation, not physical hardware.

Glimm uses its framework-agnostic createShader/playSweep API for a sweep confined to the chat composer during a reply, and for the lazy generative UI/chart placeholders. It never delays or replaces streamed text. A neutral ink palette avoids giving vault colours a new meaning. No global provider, navigation interception, page snapshots or experimental HTML-in-Canvas API is used. Ontology stays on skullet because dense review work does not need another sweep.

The overlay is absolute and pointer-transparent. The composer keeps the same bounds; generated blocks reserve 240px while their module loads. There is at most one active context, even if several historical blocks resolve together. Unmount and reduced motion cancel the sweep and release the context. Hidden tabs stop it. Reduced motion uses a static wash; missing/failed WebGL, data-saving, two or fewer CPU threads, slow-update displays and a non-charging battery at 20% or less use the CSS shimmer. Power APIs are optional, and only the signals exposed by the browser can be honoured. The polite Generating status sits outside the busy box.

The lifecycle test verifies six sequential chat generations with one live context while generating and zero after every reply. A browser response capture confirms glimm is absent before generation and its single 17,334 byte core chunk loads afterwards, 7,321 bytes gzip, or 7.1 KiB. Fallback tests cover disabled WebGL, shader initialisation failure, reduced motion, data-saving and low battery. The generated-block test holds its real JS chunk, then verifies the block resolves and its context is released.

## Bundle comparison

Baseline: origin/main `becded2307c4bba944e201e4ab4378565c3cc48d`. Same Bun lockfile install and production Turbopack settings before and after. `node scripts/route-sizes.mjs` sums unique synchronous client JS references and route bootstrap chunks. Gzip is computed per file, in KiB. These are build sizes, not measured HTTP transfer or a Lighthouse score. Deferred engines are excluded from initial JS and loaded when used. The original eager implementation added about 23 KiB gzip per data route, which prompted the split.

| Route | Before JS bytes | After JS bytes | Before gzip KiB | After gzip KiB | Difference KiB |
|---|---:|---:|---:|---:|---:|
| `/` | 747,740 | 757,442 | 214.0 | 217.6 | +3.6 |
| `/entities` | 829,047 | 835,700 | 239.3 | 241.5 | +2.2 |
| `/vaults` | 793,183 | 802,418 | 225.6 | 229.1 | +3.4 |
| `/documents` | 751,996 | 761,017 | 213.4 | 216.7 | +3.4 |
| `/connectors` | 800,234 | 809,180 | 226.1 | 229.5 | +3.4 |
| `/ontology` | 776,245 | 783,529 | 220.4 | 222.9 | +2.5 |
| `/ontology/schema` | 776,245 | 783,529 | 220.4 | 222.9 | +2.5 |
| `/ontology/review` | 776,245 | 783,529 | 220.4 | 222.9 | +2.5 |
| `/chat` | 927,047 | 936,359 | 266.8 | 269.9 | +3.1 |
| `/settings` | 1,156,385 | 1,166,269 | 326.3 | 330.0 | +3.6 |
| `/login` | 571,784 | 573,035 | 165.1 | 165.8 | +0.7 |

## Verification

- Frontend lint, including all eight existing chat UI numeric tests.
- `npx next typegen`, then `npx tsc --noEmit`.
- Production builds with no public URL env and with FRONTEND_URL=https://memory.example.test.
- Two metadata helper tests, including invalid values, public URL priority, private hosts and HTTPS.
- Chrome Playwright: 29 focused tests. Real frontend rendering with API responses intercepted at the browser boundary. No backend build or live service claim.
- Tests pin the new icon hashes, check all three ICO frames, served manifest/icons, opaque PNG dimensions and the circular maskable safe zone, absolute OG/Twitter URLs, eleven pending data states, a real torph animation for Review 19 to 18, reduced motion, haptic preference persistence and supported/disabled/reduced/desktop action gates. Seven additional glimm checks cover fallback, layout bounds, deferred loading and context disposal. Person-detail skeleton positioning is checked at 1280px and 390px. The private-host metadata test was also run against the no-env production build.
- Each pending screenshot is captured before releasing the API, and the busy state is checked again after capture. Screenshots were visually reviewed individually or in labelled contact sheets. Settings was checked at 320px in both themes.

Reproduce the focused tests against a production server:

```sh
FRONTEND_URL=https://memory.example.test bun run build
FRONTEND_URL=https://memory.example.test bun run start --port 3415
E2E_BASE_URL=http://localhost:3415 EXPECT_METADATA_ORIGIN=https://memory.example.test BRAND_SCREENSHOTS=/tmp/eunomia-brand-proof bunx playwright test tests/brand-and-motion.spec.ts
```

Screenshots and the count frame strip are published under [brand-and-motion on pr-screenshots](https://github.com/Cashara-io/eunomia/tree/pr-screenshots/brand-and-motion). The [v2 icon sheet](https://raw.githubusercontent.com/Cashara-io/eunomia/pr-screenshots/brand-and-motion/icons-v2.png) shows the served ICO frames and theme-aware SVG at 1x, 2x and 4x on light and dark browser tab strip mock-ups. Apple and manifest icons appear at native size, 2x and 4x. The Apple icon has an iOS-style home tile, and the maskable icon includes its circular 80% safe zone and a circular crop. These are mock-ups, not physical-device screenshots.
