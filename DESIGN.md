---
name: Eunomia
description: Shared memory for the AI apps people already use. Design system "Bauhaus Memory" (November 2026).
colors:
  canvas: "#FAFAF7"
  surface: "#FFFFFF"
  surface-raised: "#F0F0EB"
  surface-hover: "#E8E8E1"
  ink: "#171717"
  ink-dim: "#61615D"
  ink-faint: "#67675F"
  border: "#E2E2DC"
  border-strong: "#CBCBC4"
  border-field: "#8C8C84"
  grid-dot: "rgba(23, 23, 23, 0.08)"
  accent: "#171717"
  accent-hover: "#000000"
  accent-text: "#171717"
  accent-soft: "rgba(23, 23, 23, 0.06)"
  on-accent: "#FFFFFF"
  focus: "#3159B6"
  vault-personal: "#CF412F"
  vault-project: "#E8BE39"
  vault-company: "#3159B6"
  on-vault-personal: "#FFFFFF"
  on-vault-project: "#171717"
  on-vault-company: "#FFFFFF"
  good: "#2F7A4E"
  warning: "#946610"
  critical: "#B83A28"
  critical-soft: "rgba(207, 65, 47, 0.10)"
  on-critical: "#FFFFFF"
  scrim: "rgba(23, 23, 23, 0.32)"
  author-1: "#D9694A"
  author-2: "#4F7FCF"
  author-3: "#8D63B8"
  author-4: "#C9A030"
  author-5: "#C95A82"
  author-6: "#2F8F8A"
  canvas-dark: "#121211"
  surface-dark: "#1A1A18"
  surface-raised-dark: "#23231F"
  surface-hover-dark: "#2C2C27"
  ink-dark: "#F0F0EB"
  ink-dim-dark: "#A9A9A2"
  ink-faint-dark: "#94948C"
  border-dark: "rgba(240, 240, 235, 0.09)"
  border-strong-dark: "rgba(240, 240, 235, 0.16)"
  border-field-dark: "rgba(240, 240, 235, 0.36)"
  accent-dark: "#F0F0EB"
  on-accent-dark: "#171717"
  focus-dark: "#7C9BE6"
  vault-personal-dark: "#DD5743"
  vault-project-dark: "#ECC64C"
  vault-company-dark: "#5A80D8"
  on-vault-dark: "#171717"
  critical-dark: "#EC7A68"
typography:
  family: "Geist (next/font/google), Geist Mono for ids, code, numbers and timestamps"
  page-title: "22px / 500 / 1.2 / -0.03em"
  connect-title: "20px / 500 / -0.03em"
  section-title: "15px / 500 / -0.015em"
  body: "13px (root 14px)"
  label: "12px / 400 / ink-faint, sentence case"
  button: "14px / 500 (13px small)"
  marketing-hero: "clamp(42px, 7.2vw, 96px) / 500 / -0.04em"
rounded:
  control: "999px (buttons, nav chips, pills, vault chips)"
  field: "10px"
  card: "12px (16px on marketing)"
  tag: "4px (author tag keeps its 1px pointer corner)"
  vault-block: "4px"
  skeleton: "5px"
icons: "Phosphor Duotone (@phosphor-icons/react, the *Icon names from /ssr), back layer filled with a solid vault colour"
---

# Eunomia design system: Bauhaus Memory

Source of truth for the web app (`frontend/`), the landing page, the blog,
diagrams and docs. Tokens live in `frontend/src/app/globals.css`; every
colour there has a light and a dark value. The rollout plan this replaced is
kept in `docs/design/retheme-brief.md` for history.

**The idea in one line:** a calm cream page, black ink, and three flat colours
that each mean one kind of memory: red is personal, yellow is project, blue is
company.

**Feel:** printed, warm, confident, a little playful. Playful in empty space,
very quiet where people work.
**Not:** neon, glassy, gradient, techy dark mode, startup purple.

## Principles

1. **Make the next step obvious.** One primary action per screen. People
   should not need to understand MCP to get started.
2. **Plain words.** Our users are not technical. Say what happens, in the
   words they use, and explain consequences before they commit.
3. **Status in words.** A coloured shape may support a status; it never
   replaces the word ("Connected", "Needs attention", "Importing…").
4. **Honest state.** Success comes only from the server's response. A copied
   command or URL is not proof of a connection. No fabricated progress.
5. **Recognition supports labels.** Icons and provider marks sit beside their
   visible names; they are hidden from screen readers when redundant.
6. **Colour means something.** Red, yellow and blue are vaults and nothing
   else. Ink says "press me" and "selected"; blue focus says "you are here".
7. **Shapes live in empty space.** Empty states, onboarding, auth and banners
   get the shape compositions. Tables, forms and the palette never do.

## Language

Visible labels use these names. Routes and API names stay as they were.

| Visible label | Route | API / tools |
|---|---|---|
| Home | `/` (+ `/connect/<app>`, `/connect/help`) | (none) |
| Chat | `/chat` | (none) |
| People & topics | `/entities` | entities |
| Memory spaces | `/vaults` | vaults |
| Documents | `/documents` | documents |
| Connected sources | `/connectors` | connectors, sources |
| Help | `/docs` | docs |
| Settings | `/settings?tab=…` | (none) |
| AI instructions *(Advanced)* | `/skill` | memory skill |
| Code map *(Advanced)* | `/code` | code entities |

"Entities", "Vaults", "Connectors", "Docs", "Memory skill" and "Dashboard" are
not user-facing words. In the README and docs, the technical term may follow
in brackets ("memory spaces (vaults)").

## Colour

### Neutrals
- **Canvas** (`--canvas`): the cream page ground under a faint 20px ink dot
  grid (`.canvas-grid`). Never pure white.
- **Surface** (`--surface`): white ledgers, panels, cards, fields, the sidebar.
- **Surface raised** (`--surface-raised`): pills, hovered rows, code blocks,
  tooltips, and the ground behind decorative shapes.
- **Surface hover** (`--surface-hover`): hover on raised things, pressed
  filters, skeleton peak.
- **Ink / ink dim / ink faint:** main text; secondary text; labels,
  placeholders and table headers.
- **Border / border strong / border field:** hairlines; pill and kbd edges;
  field edges. `--border-field` exists because `--border-strong` (1.6:1) is too
  faint to find an input by; the field edge clears 3:1 in both themes.

### Accent: ink
- **Accent** (`--accent`): ink. The primary button, the selected tab, the
  selection frame and its handles, the current nav chip. Dark theme: cream with
  ink text.
- **Accent text** (`--accent-text`): ink, always underlined. Links.
- **Accent wash** (`--accent-soft`): ink at 6%, behind a selected row.
- **Focus** (`--focus`): blue, the 2px ring at 2px offset. Never selection.

### The vault colours (the signature)

| Token | Light | Dark | Text on it | Shape | Means |
|---|---|---|---|---|---|
| `--vault-personal` | `#CF412F` | `#DD5743` | white / ink | circle | Personal vault, "mine". |
| `--vault-project` | `#E8BE39` | `#ECC64C` | ink | square | Project vault, "ours, for this work". |
| `--vault-company` | `#3159B6` | `#5A80D8` | white / ink | triangle | Company vault, "everyone's". |

Always flat, solid fills: no tints, gradients or glows. Each is paired with
its shape, and the shape stands in for the colour where colour alone is not
enough. The server has personal and shared ("org") vaults today: a shared
space takes the company identity (`vaultTone()` in `components/VaultChip.tsx`).
Anything else is an ink outline square, never a fourth colour.

### Status
Deeper than the vault fills, and always with a word or icon so it is never
read as a vault: `--good` `#2F7A4E` (the only green left), `--warning`,
`--critical` with `--critical-soft` and `--on-critical`.

### Authors (presence)
`--author-1`…`--author-6` (coral, sky, plum, ochre, rose, teal) mark who wrote
something and nothing else. Never the exact vault hues. Text on each tag comes
from `--on-author-N`: ink where ink passes 4.5:1, white on plum; ink on all of
them in dark. Entity kind, connector and chart series hues stay muted.

### Contrast
Text 4.5:1, icons and large text 3:1, checked in both themes for every pairing
above. Tuned on the way in: light `--ink-faint` is `#67675F` (was `#6F6F68`,
4.4:1 on raised), light `--warning` `#946610`, dark `--ink-faint` `#94948C`,
author tag text as above, and dark vault chips take ink text (white on the
dark red and blue fell under 4.5:1).

### Named rules
**The Ink Rule.** "Press me" and "selected" are ink. Colour never says "this is a button".
**The Vault Colour Rule.** Red, yellow and blue mean personal, project and company. Never on buttons, links, status or headings.
**The Blue Focus Rule.** Focus is blue, selection is ink. They never share a colour.
**The Presence Rule.** Author colours mark who wrote something. Never invent an author.
**The Quiet Filter Rule.** Multi-select filters that are on go neutral (raised fill, ink edge). Only single choice takes the accent.
**The Two Theme Rule.** Every colour is a token with a light and a dark value. Never hard-code a colour in a component.
**The No Numbers Rule.** Section labels are words, never "01 / The project".

## Themes

Light is the default. Dark is "ink paper": warm near-black, not blue-black,
opt-in and saved per browser (`localStorage["eunomia-theme"] = "dark"`). An
inline script in `app/layout.tsx` sets `data-theme="dark"` on `<html>` before
first paint; `ThemeToggle` (sidebar footer) flips it. `color-scheme` follows
the theme so native controls match.

## Typography

One family, Geist, with `ss01` and `cv11` on; Geist Mono only for ids, code,
numbers and timestamps (always through `.font-mono`, which resets the
stylistic sets and turns on tabular numerals).

- **Headings are big and calm:** weight 500, tight tracking (`-0.04em` at 40px
  and up, `-0.03em` at 22 to 39px, `-0.015em` below).
- **App sizes stay dense:** 13px body, 14px root, `.page-title` 22px,
  `.connect-title` 20px, `.section-title` 15px, `.label` 12px `ink-faint`.
- **Marketing sizes:** hero `clamp(42px, 7.2vw, 96px)`, section titles
  `clamp(38px, 4.8vw, 68px)`. Small uppercase labels are a marketing-only
  device; the app is sentence case everywhere (`.caps` is sentence case too).
- **The No Kicker Rule.** Never a small label above a heading.
- **The Honest Mono Rule.** Mono for data that must align or be copied exactly.

## Shape and the motif

Round where you press, square where you read. Buttons, nav chips, pills and
vault chips are full pills; fields 10px; ledgers, panels and cards 12px (16px
on marketing); tags and kbd keys 4px; vault blocks 4px; skeletons 5px. Every
rule is a 1px hairline.

**The shape trio** is the motif: a red circle, a yellow square and a blue
triangle in a row, one unit apart. It is:
- the brand mark (`components/EunomiaMark.tsx`): 20px in the sidebar, 28px on
  auth and onboarding;
- the source of the sidebar nav glyphs;
- a quiet signature in the corner of diagrams and blog images;
- large and cropped by the container edge in empty states, onboarding and auth
  (`components/Shapes.tsx`).

Shapes are flat fills with no outline, shadow or gradient. They never sit
behind text or inside tables and forms, and they only move on hover (or settle
in once as a surface's one moment).

## Elevation

Flat. In-flow things (ledgers, cards) are a hairline and no shadow. Floating
things (menus, popovers, the palette, the inspector, dialogs) get one soft
shadow that starts with a 1px ring: `--shadow-panel` and `--shadow-pop`. No
glass, no blur. Never nest a card inside a card.

## Layout

- **App shell:** a 232px white sidebar with a right hairline
  (`components/Sidebar.tsx`) and a `main` canvas with the dot grid. Below 768px
  the sidebar becomes a 56px top bar and a slide-in drawer (`role="dialog"`,
  Escape closes it, focus moves in and back).
- **Page header** (`components/ui/PageHeader.tsx`): a 26px Phosphor Duotone
  icon (back-layer colour varied per page with `tone`) or a provider mark, the
  page title, optional actions on the right, a hairline, then one introduction
  sentence.
- **Task panel** (`.task-panel`): a 1px ink frame around the one thing to do on
  a screen (the app picker, a connection guide, onboarding).
- Pages cap their width (settings and connectors 64rem, documents 72rem) and
  stack sections 24 to 36px apart. More space above a heading than below it.

## Components

### Buttons (`.btn`)
- **Primary** (`.btn-primary`): ink pill, white text, 36px tall, 16px side
  padding, weight 500. Hover goes to pure black (white in dark). One per view.
- **Secondary** (the default `.btn`): transparent pill with a 1px ink edge.
  Hover fills `surface-raised`.
- **Ghost** (`.btn-ghost`): no edge, hover fills `surface-raised`.
- **Danger** (`.btn-danger`): transparent, `critical` text and edge, hover
  `critical-soft`. The final confirm is `.btn-danger-solid`. Destructive vault
  and token actions use `HoldButton`.
- **Small** (`.btn-sm`) 30px; icon-only (`.btn-icon`) is square. 44px on
  coarse pointers. Press scales to 0.97 over 140ms.

### Nav chips (sidebar)
Pills, 32px tall (44px on touch), with a small glyph built from the three
shapes before the label. Each section has its own glyph (`data-glyph` in
`globals.css`): Home is a house, Chat two bubbles, People & topics a person
and a triangle, Memory spaces the trio itself, Documents the landing page's
yellow page with a red half moon, and so on. Hover fills `surface-raised`,
the edge turns ink and the shapes rearrange (320ms,
`cubic-bezier(0.2, 0.7, 0.2, 1)`), only with a fine pointer and never under
reduced motion. The current page (`aria-current="page"`) is an ink chip. At
rest the edge is transparent: a column of ten outlined pills is too busy for a
working sidebar.

### Vault chips and rows (`components/VaultChip.tsx`)
The one way to name a vault.
- **VaultChip:** a pill in the vault's solid colour, its shape at 8px before
  the name in the chip's text colour, the kind in words for screen readers.
  Used for invitations, token scope, extension and connector destinations.
- **VaultShape:** the 12px shape before a name in a list row (memory spaces
  list and detail, vector cloud layers and rows). Never tint the whole row.

### Fields (`.field`, `components/ui/Field.tsx`)
White fill, 1px `--border-field` edge, 10px radius, 36px tall. On focus the
edge turns ink and a 2px blue ring sits outside it. Persistent visible label,
hint and inline error tied with `aria-describedby`; required stated in words.

### Pills (`.pill`)
Fully round, 32px. Filters (`aria-pressed`) go neutral when on; single choice
(`aria-selected`, `aria-checked`, `aria-current`) is an ink fill.

### Selection frame
A 1px ink outline plus four 6px ink handles. A selected table row gets the
outline and the `accent-soft` wash. Handles snap in when a pointer picked the
frame; keyboard selection shows them at once.

### Data tables (`.data-table`)
13px, tabular numerals, 32px sentence-case headers in 12px `ink-faint`, 40px
rows, hairline rules, row hover `surface-raised`. Secondary columns hide on
small screens rather than wrap.

### Status, loading, empty, error
- **StatusLabel** (`components/ui/StatusLabel.tsx`): a word plus a small dot
  in `good`, `warning`, `critical`, ink (info) or an outline (neutral).
- **Skeleton:** `.skeleton`, shaped like the content. `null` is loading, an
  empty array is empty; never show the empty message while loading.
- **Empty state** (`EmptyState` in `components/Shapes.tsx`): one composition
  of two or three flat shapes in a raised panel, cropped by its edge, beside
  one sentence and one action. Inside a table, a single plain row instead.
- **ErrorLine:** `role="alert"`, `critical` text on `critical-soft`, 10px
  radius, what failed and what to do next.
- **Confirmation** (`components/ui/ConfirmDialog.tsx`): native `<dialog>` that
  names the target in the title, states the consequence, focuses Cancel first,
  can lock behind typing the name, and returns focus to the opener.

### Code to copy
`CommandBlock` in `components/connect/Guide.tsx`: a raised mono block with a
hairline, then one primary "Copy command" button.

### Tooltip, kbd, select, palette
- **Tooltip** (`components/bits/Tooltip.tsx`): `surface-raised`, 6px radius,
  `--shadow-pop`, 12px text. Visual only.
- **Kbd** (`.kbd`): 4px radius, mono 11px `ink-faint`.
- **Select** (`components/Select.tsx`): the only select; a `.field` trigger,
  a portalled `.select-list` with `--shadow-pop`.
- **Command palette:** a dialog over the scrim, Phosphor icons per row (the
  palette is a dense list, so no shapes), raised fill on the active row. It
  never animates.

### Icons
**Phosphor Duotone everywhere** (`@phosphor-icons/react`, MIT). Import the
`*Icon` names from `@phosphor-icons/react/ssr` so server and client components
both work, always with `weight="duotone"`.
- **The Eunomia twist:** the back layer is a solid vault colour instead of a
  20% tint; the line stays the text colour. `globals.css` fills every Phosphor
  duotone layer with `var(--duo)` (yellow by default); set `--duo` per icon, or
  use `components/ui/Duo.tsx` with `tone="personal" | "project" | "company"`.
  Vary the tones across a row so neighbours differ; pick for meaning when there
  is one. On a vault-coloured tile the back layer is white (`tone="white"`).
- **Status and danger keep the line only:** icons inside `.btn-danger`,
  `[role="alert"]` and `.status-label` get a transparent back layer.
- **Sizes:** 16px in dense UI, 20 to 26px in tiles and headers, 28px and up in
  empty states.
- **Agent brand marks** (Claude, ChatGPT, Codex, Cursor, Gemini and more) come
  from `components/marks/AssistantMarks.tsx` (Simple Icons, CC0, and LobeHub,
  MIT), a factual "connects to" reference beside the visible name. Lucide is
  gone; do not add it back.

### Brand mark and theme toggle
- **EunomiaMark** (`size`, default 20): the shape trio from the vault tokens,
  so it follows the theme.
- **ThemeToggle** (`components/ThemeToggle.tsx`): a ghost icon button with
  `aria-pressed`, Sun and Moon duotone icons.

## Motion

Tokens: `--ease-out` cubic-bezier(0.23, 1, 0.32, 1) for entrances and presses;
`--ease-in-out` for things moving on screen; `--ease-drawer` for the drawer;
`--ease-shape` cubic-bezier(0.2, 0.7, 0.2, 1) for shapes moving on hover.
Durations: hover 120ms, press 140ms, pop 180ms, drawer 260ms, shape 320ms.

- **Shapes move on hover only,** under 400ms. The one exception is a surface's
  single authored moment: a shape composition settling in on an empty state,
  auth or an onboarding step (420ms, staggered 70ms, once).
- **The One Moment Rule.** At most one authored moment per surface. The
  onboarding welcome keeps `BlurWords` on its heading, so its shapes stay still.
- Nothing animates on the palette, sidebar navigation, tab switches, typing or
  keyboard list movement. Hover styles live only inside
  `(hover: hover) and (pointer: fine)`. Popovers enter from scale 0.96, never
  from nothing.
- **Reduced motion:** transitions shrink to opacity and colour, keyframes stop,
  shapes stay put, final states render at once.
- **Honest text:** the final text is in the DOM from the first frame.

The bits library (`frontend/src/components/bits/`, styles in `bits.css`):
SyncMark, CopyButton, DigitRoll, DecryptReveal, HoldButton, Spark, BlurWords,
Tooltip. Each file is an original implementation inspired by React Bits
(reactbits.dev); no React Bits source is copied (**The Licence Rule**).

## Connection flow (Home)

1. **Choose the AI app.** Home (`/`) asks "Which AI app do you use?" with
   ChatGPT, Claude Code, Codex and Other apps, each with its own mark. Nothing
   is preselected. Below: "Or start with your own information".
2. **Follow that app's steps** (`/connect/<app>`). Browser setup (ChatGPT) and
   terminal setup (Claude Code, Codex: Mac / Windows / Linux) are separate.
   Commands match `docs/agents.md` (OAuth, no pasted token). An *Advanced*
   section keeps the API-token method.
3. **Sign in and approve** on the consent page, which explains the access in
   plain words before Allow.
4. **Confirm and try it.** "Check connection" asks the server. Success appears
   only when Eunomia has an approved sign-in for that app or a token it has
   actually used.

Connection help (`/connect/help`) is linked from every guide.

## Sources: connect, then import

Keep connection status separate from imported-content status: choose source,
connection checked, ready to import (destination and scope shown), importing
(honest status), imported (count and a way to inspect). Sign-in failures and
import failures have different recovery actions. Destination changes affect
future imports; moving existing items is a separate, confirmed action.

## Memory spaces

Explain scope before people commit. Copy creates a new space; combining two
spaces creates a new combined space and leaves both originals unchanged.
Pending invitations grant no access. Show the person's role and enforce the
last-owner rule. Delete, leave and move name the target and consequence and
offer Cancel.

## Accessibility and responsive contract

WCAG 2.2 AA.
- Every action keyboard operable; visible blue focus, distinct from ink
  selection.
- 44px touch targets on coarse pointers; check 200% zoom and 375px widths; no
  horizontal page overflow.
- Dialogs manage focus and return it to the invoking control.
- Persistent labels, inline errors with correction guidance.
- Announce meaningful completion and errors (`aria-live`), not per-frame
  updates. Decorative shapes and glyphs are `aria-hidden`.
- Browser surfaces carry the design: ink selection, ink caret, thin
  `border-strong` scrollbars, ink checkboxes (`accent-color`), 3px link
  underline offset.
- Minimise exposed secrets: tokens are masked, a new token is shown once.

## Not part of the released product

Do not design or build these as if they exist: a hosted script editor for
extensions, a skill-library screen, Pocket original audio, hosted extension
runtimes and universal instance sync.

## Do and don't

**Do**
- Build from the tokens and classes in `globals.css`; add a token (with a dark
  value) rather than a raw hex.
- Keep the page cream and the work areas white and quiet.
- Pair every vault colour with its shape; name vaults with `VaultChip`.
- Put the playfulness in empty space.
- Check every screen in both themes, at desktop and 375px.

**Don't**
- Use the vault colours for buttons, links, status or decoration that is not
  the motif, or tint, fade or gradient them.
- Add a fourth brand colour, or bring back the old felt green.
- Put shapes behind text or inside tables and forms.
- Number section labels, or set a kicker above a heading.
- Give in-flow blocks a shadow, use glass, blur or glows.
- Mix icon sets, or use emoji and unicode glyphs as icons.
- Use em dashes in UI copy.
