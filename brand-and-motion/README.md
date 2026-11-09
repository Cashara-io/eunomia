# Eunomia brand and motion proof

Original Eunomia artwork and synthetic frontend fixtures only. Every image was visually inspected before upload.

- icons.png: served ICO frames at 16/32/48, theme-aware SVG, Apple 180 and manifest 192/512. This sheet shows actual native image sizes, not browser chrome.
- opengraph-image.png and twitter-image.png: static 1200x630 previews.
- skeleton-*.png: pending API responses, with the loading state checked both before and after capture.
- count-strip.png and count-frame-*.png: real torph Review count change, 19 to 18. The spec checks an active Web Animation before capture.
- haptics-*-320.png: Accessibility at 320px in both themes. Physical phone vibration is not claimed.
- count-reduced-after.png and review-reduced.png: reduced motion uses static text.
- glimm-strip.png and glimm-frame-*.png: the real neutral glimm sweep during chat generation.
- glimm-fallback-*.png: disabled WebGL, initialisation failure, reduced motion, data-saving and low-battery fallbacks.
- glimm-generated-block.png: a real deferred UI module held pending.
- glimm-lazy-chunk.json: the deferred core response size, 7.1 KiB gzip, absent before the first generation.
- Six sequential generations each returned to zero live WebGL contexts.

29 Chrome Playwright tests passed. Public URL and private request-host builds both passed. Main build used FRONTEND_URL=https://memory.example.test. See frontend/docs/brand-and-motion.md on the feature branch for the full bundle table and reproduction commands.
