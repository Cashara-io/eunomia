# Eunomia re-theme brief: "Bauhaus Memory"

Status: approved direction, November 2026. The landing page (`/eunomia` on the Build Paperless site) already ships it. The Eunomia app (`frontend/`, Next.js) still ships the old felt-green theme. This brief is the plan to move the app across.

`DESIGN.md` at the repo root now describes this system, so the impeccable skill and any agent working on the UI uses it. Until the rollout below is done, the code and `DESIGN.md` disagree on colour. When they disagree, follow this brief.

## 1. The idea in one line

A calm cream page, black ink, and three flat colours that each mean one kind of memory: red is personal, yellow is project, blue is company.

The old app looked like a dark design tool (Figma, tldraw) with one green accent. The new look is lighter and friendlier: printed paper, a Bauhaus poster, simple shapes. It should feel playful in the empty spaces and very quiet where people work.

**Feel:** printed, warm, confident, a little playful.
**Not:** neon, glassy, gradient, techy dark mode, startup purple.

## 2. Palette

Every colour is a CSS variable. Variable names in `frontend/src/app/globals.css` stay the same where they exist, so most of the switch is changing values, not renaming.

### 2.1 Light (the new default)

| Token | Value | Use |
|---|---|---|
| `--canvas` | `#FAFAF7` | Page ground. Warm off-white, never pure white. |
| `--surface` | `#FFFFFF` | Ledgers, panels, cards, sidebar. |
| `--surface-raised` | `#F0F0EB` | Buttons at rest, fields, pills, hovered rows, code. The landing page calls this "soft". |
| `--surface-hover` | `#E8E8E1` | Hover on raised things, pressed filters, skeleton peak. |
| `--ink` | `#171717` | Main text, primary button fill, selection outline. |
| `--ink-dim` | `#61615D` | Secondary text, descriptions. |
| `--ink-faint` | `#6F6F68` | Labels, placeholders, table headers. Tuned to pass 4.5:1 on canvas. |
| `--border` | `#E2E2DC` | Hairlines and container edges. |
| `--border-strong` | `#CBCBC4` | Button and field edges, scrollbar thumb. |
| `--grid-dot` | `rgba(23, 23, 23, 0.08)` | The 20px dot grid on the canvas. |
| `--scrim` | `rgba(23, 23, 23, 0.32)` | Behind the palette and drawer. |

### 2.2 The three vault colours (the signature)

| Token | Value | Text on it | Shape | Means |
|---|---|---|---|---|
| `--vault-personal` | `#CF412F` | `#FFFFFF` | circle | Personal vault, "mine". |
| `--vault-project` | `#E8BE39` | `#171717` | square | Project vault, "ours, for this work". |
| `--vault-company` | `#3159B6` | `#FFFFFF` | triangle | Company vault, "everyone's". |

These three are the brand. They are always flat, solid fills. No tints, no gradients, no glows. Each one is paired with a shape, and the shape can stand in for the colour when colour alone would not be enough (colour-blind users, greyscale print).

### 2.3 Accent and focus

| Token | Old value | New value | Use |
|---|---|---|---|
| `--accent` | felt green `#3fa873` | `#171717` | Primary button fill, selected tab, selection frame, active nav marker. |
| `--on-accent` | `#04140b` | `#FFFFFF` | Text and icons on an accent fill. |
| `--accent-text` | `#52bd87` | `#171717` | Links. Always underlined, so they do not need colour to read as links. |
| `--accent-soft` | green 14% | `rgba(23, 23, 23, 0.06)` | Selected row and selected pill wash. |
| `--focus` (new) | none | `#3159B6` | The 2px focus ring. Blue, so it never looks like a selected (black) state. |

The accent is ink. The landing page's main button is a black pill with white text ("Get started"), and the app follows it. Colour is saved for meaning (vaults, status, authors), not for "press me".

### 2.4 Status

Status must not be confused with a vault, so status always comes with an icon or a word, and text uses deeper tones than the vault fills.

| Token | Value | Use |
|---|---|---|
| `--good` | `#2F7A4E` | Healthy dot, drawn done check. The only green left in the product. |
| `--warning` | `#9A6B12` | Warning text and dot. |
| `--critical` | `#B83A28` | Error text, danger buttons, hold-to-delete fill. |
| `--critical-soft` | `rgba(207, 65, 47, 0.10)` | Error line and banner wash. |

### 2.5 Authors (presence palette)

Author tags keep their job (who wrote this) but move into the same warm, printed family. They must never use the exact vault hues, so an author tag is never read as a vault.

| Slot | Old | New |
|---|---|---|
| `--author-1` | `#e0623c` | `#D9694A` coral |
| `--author-2` | `#2f7fd8` | `#4F7FCF` sky |
| `--author-3` | `#8a5ad8` | `#8D63B8` plum |
| `--author-4` | `#c48a14` | `#C9A030` ochre |
| `--author-5` | `#cf4f84` | `#C95A82` rose |
| `--author-6` | `#1f9aa1` | `#2F8F8A` teal |

Text on an author tag is `#FFFFFF` except on ochre, which takes `#171717`.

Entity kind hues, connector hues and chart series keep their current muted values. They already sit well on cream.

### 2.6 Dark theme

The landing page is light only. The app keeps a dark theme, but light becomes the default. Dark is "ink paper": warm near-black, not blue-black.

| Token | Dark value |
|---|---|
| `--canvas` | `#121211` |
| `--surface` | `#1A1A18` |
| `--surface-raised` | `#23231F` |
| `--surface-hover` | `#2C2C27` |
| `--ink` | `#F0F0EB` |
| `--ink-dim` | `#A9A9A2` |
| `--ink-faint` | `#8A8A83` |
| `--border` | `rgba(240, 240, 235, 0.09)` |
| `--border-strong` | `rgba(240, 240, 235, 0.16)` |
| `--accent` | `#F0F0EB` (cream pill, ink text) |
| `--on-accent` | `#171717` |
| `--focus` | `#7C9BE6` |
| `--vault-personal` | `#DD5743` |
| `--vault-project` | `#ECC64C` |
| `--vault-company` | `#5A80D8`, with `#171717` text |

Check every pairing for 4.5:1 text contrast (3:1 for large text and icons) before shipping. The values above are the starting point, not a promise.

## 3. Type

One family, Geist, as today. What changes is the voice of the big type.

- **Headings:** weight 500, not 600. Tight tracking: `-0.04em` at 40px and up, `-0.03em` at 22 to 39px, `-0.015em` below. Big and calm beats bold and heavy.
- **App sizes stay dense:** 13px body, 14px root, 22px page title. Only the weight and tracking of headings change.
- **Marketing sizes:** hero `clamp(42px, 7.2vw, 96px)`, section titles `clamp(38px, 4.8vw, 68px)`, as the landing page does.
- **Labels:** the landing page uses small uppercase labels (11px, weight 500, `0.12em` tracking) above sections. The app keeps sentence-case labels and keeps its No Kicker Rule. Uppercase labels are a marketing-only device.
- **No numbered labels.** Never "01 / The project". The label is just "The project". Numbers in labels read as a slide deck, not a product.
- **Mono:** Geist Mono, only for ids, code, numbers and timestamps, as today.

## 4. Shape and the motif

### 4.1 Radii

| Thing | Old | New |
|---|---|---|
| Buttons | 7px | Fully round pill (`999px`). |
| Nav chips | 7px rows | Fully round pill. |
| Fields | 7px | 10px. |
| Pills, tags, tooltips | 6px | Fully round for pills; tags keep 4px with the 1px pointer corner. |
| Ledgers, panels, cards | 10px | 12px in the app, 16px on marketing. |
| Vault blocks | none | 4px. They should look like printed blocks, nearly square. |

### 4.2 The shape trio

The motif is three flat shapes in a row: a red circle, a yellow square and a blue triangle. It is the personal, project and company vaults as one mark.

Use it:
- As the **brand mark**, replacing the green tile with three nodes. Size it at 20px in the sidebar and 28px in auth and onboarding.
- In the **nav chips**: a small group of shapes that moves on hover (see 5.2).
- In the corner of **diagrams and blog images**, as a quiet signature.
- Large and cropped in **empty states, onboarding and auth**: big circles, half circles and triangles running off the edge, like the blog banners.

Do not use it:
- Inside dense work areas (tables, forms, the palette). Shapes are for empty space, not busy space.
- With gradients, shadows, outlines or rotation animations that loop forever.

### 4.3 Elevation

Flatter than before. In-flow things (ledgers, cards) have a hairline and no shadow. Floating things (menus, popovers, the palette, the inspector) get one soft shadow: `0 0 0 1px var(--border), 0 12px 32px -12px rgba(23, 23, 23, 0.18)`. No glass, no blur.

## 5. Components

### 5.1 Buttons

- **Primary:** ink pill, white text, 36px tall in the app (was 30px), 16px side padding, weight 500. Hover darkens to pure `#000`. One per view.
- **Secondary:** transparent pill with a 1px ink border, ink text. Hover fills `--surface-raised`.
- **Ghost:** no border, ink text, hover fills `--surface-raised`.
- **Danger:** transparent pill, `--critical` text and border, hover fills `--critical-soft`.
- Press stays the same: scale 0.97 over 140ms.

### 5.2 Nav chips (new, from the landing page)

Pills with a small group of Bauhaus shapes in front of the label. The shapes rearrange on hover. Built first in the landing page's `NavChip` component (`src/components-pages/EunomiaPage.tsx` in the paperless repo).

- 44px tall on marketing, 32px in the app sidebar. Pill radius. 1px `--border` edge on `--canvas`.
- Hover: fill `--surface-raised`, edge turns `--ink`, shapes move (320ms, `cubic-bezier(0.2, 0.7, 0.2, 1)`).
- Current page (`aria-current="page"`): ink fill, white text.
- Each section gets its own small glyph built from the three shapes. For example, "How it works" is the three shapes in a row that step up and down, and "Blog" is a yellow page with a red half moon.
- In the app sidebar, give each main section its own glyph the same way (Memory, Vaults, Entities, Connectors, Chat, Settings). Shapes sit at 14px and only move on hover with a fine pointer.
- Reduced motion: no movement, shapes stay put.

### 5.3 Vault chips and rows

Wherever the app shows which vault something belongs to, use the vault colour as a flat fill with its shape:

- **Vault chip:** pill, solid vault colour, the matching shape at 8px before the name.
- **Vault row in a list:** a 4px-wide solid bar in the vault colour on the left edge, or a 12px shape before the name. Do not tint the whole row.
- **Mixed or custom vaults:** an ink outline square. Do not invent a fourth colour.

### 5.4 Selection frame

Keep the signature, recoloured: a 1px ink outline plus four 6px ink handles. The selected table row gets the `--accent-soft` wash. Focus stays blue, so selected (black) and focused (blue) never look the same.

### 5.5 Fields

`--surface` fill (white), 1px `--border-strong` edge, 10px radius, 36px tall. On focus the edge turns `--ink` and a 2px `--focus` ring sits outside it.

### 5.6 Empty states, onboarding and auth

This is where the playfulness goes.

- **Empty state:** one big composition of two or three flat shapes (for example a yellow circle cropped by the card edge and a small blue triangle), then one sentence and one action. Shapes sit in `--surface-raised` space, never behind text.
- **Onboarding:** each step gets one shape of its own. The welcome step shows all three together.
- **Auth:** the left half is a large Bauhaus composition in the three vault colours on `--surface-raised`. The form sits on white.

### 5.7 Charts and diagrams

- Diagrams follow the blog images: `--surface-raised` panel, white cards with hairline edges, ink connectors with chevron arrows, vault colours for the one thing that matters.
- Charts use the vault colours for vault series, and the muted series hues for everything else.

### 5.8 Icons

**Phosphor Duotone is the icon set for every Eunomia surface:** the app, the landing page, the blog, diagrams and docs. It replaces Lucide.

- Package: `@phosphor-icons/react`, always `weight="duotone"`. MIT licence, 9,000+ icons, so we will not run out.
- The Eunomia twist: Phosphor's back layer is normally a 20% tint. We fill it with a solid vault colour (red, yellow or blue) and keep the line ink. CSS: `.duo path[opacity] { opacity: 1; fill: var(--duo); }`.
- On a vault-coloured tile, the back layer is white.
- Agent brand logos (Claude, Codex, Cursor) stay as `AssistantMarks`.
- Rollout step: swap `lucide-react` imports screen by screen during step 2 of the plan below, then remove the package.

## 6. Motion

The motion rules in `DESIGN.md` stay. Two additions:

- **Shapes move on hover only,** under 400ms, with the landing page's ease `cubic-bezier(0.2, 0.7, 0.2, 1)`. The one exception is a section's single authored moment, such as the shapes travelling along the How it works connectors: a slow loop, desktop only, switched off under reduced motion.
- **One moment per surface still holds.** A shape composition that animates in on an empty state counts as that surface's moment.

## 7. Mapping the app to the new tokens

Most of the work is swapping values in `frontend/src/app/globals.css`. This table lists the rest.

| Today in the app | Becomes |
|---|---|
| Dark theme by default | Light by default. Dark stays, opt-in, saved per user. |
| Felt green primary button | Ink pill. |
| Green focus ring | Blue focus ring (`--focus`). |
| Green selected row wash | Ink 6% wash. |
| Green selection frame and handles | Ink frame and handles. |
| Green brand tile with three nodes | Shape trio mark. |
| Sidebar nav rows | Nav chips with per-section glyphs. |
| Green "Connect one" links | Ink, underlined. |
| `--good` (was the same green as the accent) | Its own `#2F7A4E`, status only. |
| Vault names as plain text | Vault chips with colour and shape. |
| 7px buttons, 10px cards | Pill buttons, 12px cards. |

## 8. Rollout for the app

Do it in this order. Each step is one small PR that leaves the app working.

1. **Tokens.** Change the values in `globals.css` for light and dark. Add `--focus` and the three `--vault-*` tokens. Make light the default in `layout.tsx` (the saved-theme script flips it). No component changes. Check every page in both themes.
2. **Buttons, fields and radii.** Update `.btn`, `.field`, `.pill`, `.ledger`, `.panel` and the radius tokens. Replace the focus ring colour.
3. **Brand mark and sidebar.** Swap `EunomiaMark` for the shape trio. Turn the sidebar nav rows into nav chips with glyphs.
4. **Vault chips.** Add one `VaultChip` component and use it everywhere a vault is named (memory rows, vault list, entity detail, chat sources).
5. **Empty states, onboarding and auth.** Add the shape compositions. This is the playful step, and the riskiest for taste, so review it with screenshots before merging.
6. **Clean-up.** Remove every leftover felt-green value, update the screenshots in `docs/images`, and rewrite the felt-green parts of `DESIGN.md` from "migration" to "done".

## 9. Done means

- No `#3fa873`, `#52bd87`, `#2b7d53` or `#23744b` left in `frontend/src`.
- Every page checked in light and dark, on desktop and at 375px wide.
- Text contrast is 4.5:1 or better everywhere, and 3:1 or better for icons and large text.
- The three vault colours appear only as vault identity and in the shape motif.
- No numbered labels, no gradients, no glass, no glows.

## 10. Do and don't

**Do**
- Keep the page cream and the work areas white and quiet.
- Let colour mean something: red, yellow and blue are vaults, and nothing else.
- Pair every vault colour with its shape.
- Put the playfulness in empty space: empty states, onboarding, auth, banners.
- Use ink for "press me" and blue for "you are here" (focus).

**Don't**
- Use the vault colours for buttons, links, status or decoration that is not the motif.
- Tint, fade or gradient the vault colours.
- Add a fourth brand colour, or bring the felt green back.
- Put shapes behind text or inside tables and forms.
- Number section labels.
- Use em dashes in UI copy.
