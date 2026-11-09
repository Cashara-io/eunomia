// Original Eunomia artwork, derived from EunomiaMark.tsx and DESIGN.md. No network or next/og.
import sharp from "sharp";
import { writeFile } from "node:fs/promises";
const app = new URL("../src/app/", import.meta.url);
const rowShapes =
  '<circle cx="6" cy="6" r="6" fill="#CF412F"/><path d="M16 0h12v12H16z" fill="#E8BE39"/><path d="M38 0 44 12H32Z" fill="#3159B6"/>';
// Whole-pixel bounds at 16px. The triangle has the same visual weight as the top shapes.
const squareShapes =
  '<circle cx="4" cy="4" r="3" fill="#CF412F"/><path d="M9 1h6v6H9z" fill="#E8BE39"/><path d="M8 8 12 15H4Z" fill="#3159B6"/>';
// The furthest corner is 7 * sqrt(2) from the centre. Scaling by .62
// puts every shape inside the maskable safe circle, radius 6.4 of 16 units.
const icon = (maskable = false) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" fill="#FAFAF7"/><g${maskable ? ' transform="translate(8 8) scale(.62) translate(-8 -8)"' : ""}>${squareShapes}</g></svg>`;
await writeFile(
  new URL("icon.svg", app),
  icon().replace(
    "<rect ",
    "<style>@media(prefers-color-scheme:dark){rect{fill:#121211}circle{fill:#DD5743}path:first-of-type{fill:#ECC64C}path:last-of-type{fill:#5A80D8}}</style><rect ",
  ),
);
for (const [name, size, maskable] of [
  ["apple-icon.png", 180, false],
  ["icon1.png", 192, false],
  ["icon2.png", 512, false],
  ["../../public/maskable-icon.png", 512, true],
]) {
  await writeFile(
    new URL(name, app),
    await sharp(Buffer.from(icon(maskable))).resize(size, size).removeAlpha().png().toBuffer(),
  );
}
const frames = await Promise.all(
  [16, 32, 48].map((size) =>
    sharp(Buffer.from(icon())).resize(size, size).png().toBuffer(),
  ),
);
const header = Buffer.alloc(6 + 16 * frames.length);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(3, 4);
let offset = header.length;
frames.forEach((frame, i) => {
  const at = 6 + i * 16;
  header[at] = header[at + 1] = [16, 32, 48][i];
  header.writeUInt16LE(1, at + 4);
  header.writeUInt16LE(32, at + 6);
  header.writeUInt32LE(frame.length, at + 8);
  header.writeUInt32LE(offset, at + 12);
  offset += frame.length;
});
await writeFile(
  new URL("favicon.ico", app),
  Buffer.concat([header, ...frames]),
);
if (process.argv.includes("--icons-only")) process.exit(0);
const fontfile = new URL(
  "../node_modules/geist/dist/fonts/geist-sans/Geist-Medium.ttf",
  import.meta.url,
).pathname;
const title = await sharp({
  text: { text: '<span foreground="#171717">Eunomia</span>', font: "Geist Medium 104", fontfile, rgba: true },
})
  .png()
  .toBuffer();
const subtitle = await sharp({
  text: {
    text: '<span foreground="#171717">Shared memory for the AI apps\nyou already use</span>',
    font: "Geist Medium 36",
    fontfile,
    rgba: true,
    spacing: 10,
  },
})
  .png()
  .toBuffer();
const base = `<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630"><rect width="1200" height="630" fill="#FAFAF7"/><path d="M64 550H1136" stroke="#E2E2DC"/><g transform="translate(68 74) scale(3)">${rowShapes}</g><circle cx="1110" cy="58" r="110" fill="#CF412F"/><path d="M1040 470h190v190h-190z" fill="#E8BE39"/><path d="M1050 247 1180 472H920Z" fill="#3159B6"/></svg>`;
const og = await sharp(Buffer.from(base))
  .composite([
    { input: title, left: 64, top: 210 },
    { input: subtitle, left: 68, top: 360 },
  ])
  .png()
  .toBuffer();
for (const name of ["opengraph-image.png", "twitter-image.png"])
  await writeFile(new URL(name, app), og);
