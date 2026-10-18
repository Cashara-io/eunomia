---
name: Eunomia
description: Shared long-term memory for your team's AI agents, drawn as a multiplayer canvas.
colors:
  canvas: "#0b0c0e"
  surface: "#121316"
  surface-raised: "#1a1b1f"
  surface-hover: "#212328"
  ink: "#ededef"
  ink-dim: "#a3a7ae"
  ink-faint: "#80858e"
  border: "rgba(237, 237, 239, 0.08)"
  border-strong: "rgba(237, 237, 239, 0.15)"
  grid-dot: "rgba(237, 237, 239, 0.07)"
  felt-green: "#3fa873"
  felt-green-text: "#52bd87"
  felt-green-soft: "rgba(63, 168, 115, 0.14)"
  on-accent: "#04140b"
  good: "#3fa873"
  warning: "#d0a651"
  critical: "#d4705f"
  critical-soft: "rgba(212, 112, 95, 0.12)"
  scrim: "rgba(4, 5, 6, 0.6)"
  author-1: "#f0815e"
  author-2: "#5fa6f2"
  author-3: "#b48cf0"
  author-4: "#e2b04f"
  author-5: "#e77fa9"
  author-6: "#4fc1c7"
  on-author: "#0b0c0e"
  kind-person: "#6f9bc9"
  kind-organisation: "#a692c9"
  kind-location: "#c98a5a"
  kind-repository: "#5a9e8f"
  kind-file: "#9e9150"
  kind-symbol: "#9e5a7a"
  canvas-light: "#f5f6f7"
  surface-light: "#ffffff"
  surface-raised-light: "#f0f1f3"
  surface-hover-light: "#e8e9ec"
  ink-light: "#111214"
  ink-dim-light: "#4f545c"
  ink-faint-light: "#676c75"
  border-light: "rgba(17, 18, 20, 0.1)"
  border-strong-light: "rgba(17, 18, 20, 0.18)"
  grid-dot-light: "rgba(17, 18, 20, 0.1)"
  felt-green-light: "#2b7d53"
  felt-green-text-light: "#23744b"
  felt-green-soft-light: "rgba(43, 125, 83, 0.1)"
  on-accent-light: "#ffffff"
  warning-light: "#9a6b12"
  critical-light: "#b4432f"
  critical-soft-light: "rgba(180, 67, 47, 0.08)"
  scrim-light: "rgba(17, 18, 20, 0.28)"
  author-1-light: "#e0623c"
  author-2-light: "#2f7fd8"
  author-3-light: "#8a5ad8"
  author-4-light: "#c48a14"
  author-5-light: "#cf4f84"
  author-6-light: "#1f9aa1"
  on-author-light: "#ffffff"
typography:
  display:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "22px"
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "-0.025em"
  headline:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "17px"
    fontWeight: 600
    letterSpacing: "-0.015em"
  title:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 600
    letterSpacing: "-0.005em"
  body:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    fontFeature: "\"ss01\", \"cv11\""
  body-root:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    fontFeature: "\"ss01\", \"cv11\""
  label:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.3
  stat:
    fontFamily: "Geist Mono, ui-monospace, monospace"
    fontSize: "26px"
    lineHeight: 1
    letterSpacing: "-0.02em"
    fontFeature: "\"tnum\""
  mono:
    fontFamily: "Geist Mono, ui-monospace, monospace"
    fontSize: "12.5px"
    fontWeight: 400
    fontFeature: "\"tnum\""
  tag:
    fontFamily: "Geist, system-ui, sans-serif"
    fontSize: "11.5px"
    fontWeight: 500
    lineHeight: 1
rounded:
  skeleton: "5px"
  tag: "4px"
  option: "6px"
  pill: "6px"
  control: "7px"
  field: "7px"
  code: "8px"
  card: "10px"
  full: "999px"
spacing:
  hair: "1px"
  "1": "4px"
  "1.5": "6px"
  "2": "8px"
  "3": "12px"
  "4": "16px"
  "5": "20px"
  "6": "24px"
  "8": "32px"
  "10": "40px"
components:
  button:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-hover:
    backgroundColor: "{colors.surface-hover}"
  button-primary:
    backgroundColor: "{colors.felt-green}"
    textColor: "{colors.on-accent}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.ink-dim}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-ghost-hover:
    textColor: "{colors.ink}"
  button-danger:
    backgroundColor: "transparent"
    textColor: "{colors.critical}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "30px"
  button-danger-hover:
    backgroundColor: "{colors.critical-soft}"
  button-sm:
    padding: "0 9px"
    height: "26px"
  button-icon:
    width: "30px"
    padding: "0"
  field:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.field}"
    padding: "0 12px"
    height: "32px"
  pill:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.ink-dim}"
    rounded: "{rounded.pill}"
    padding: "0 9px"
    height: "24px"
  pill-pressed:
    backgroundColor: "{colors.surface-hover}"
    textColor: "{colors.ink}"
  pill-selected:
    backgroundColor: "{colors.felt-green-soft}"
    textColor: "{colors.ink}"
  author-tag:
    textColor: "{colors.on-author}"
    typography: "{typography.tag}"
    padding: "0 6px"
    height: "20px"
  kbd:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink-faint}"
    rounded: "{rounded.tag}"
    padding: "0 4px"
    height: "18px"
  select-option:
    textColor: "{colors.ink-dim}"
    rounded: "{rounded.option}"
    padding: "0 8px"
    height: "32px"
  select-option-active:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.ink}"
  error-line:
    backgroundColor: "{colors.critical-soft}"
    textColor: "{colors.critical}"
    rounded: "{rounded.field}"
    padding: "8px 12px"
  nav-link:
    textColor: "{colors.ink-dim}"
    rounded: "{rounded.control}"
    padding: "0 10px"
    height: "32px"
  nav-link-active:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.ink}"
  panel:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.card}"
  ledger:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.card}"
  tooltip:
    backgroundColor: "{colors.surface-raised}"
    textColor: "{colors.ink}"
    rounded: "{rounded.pill}"
    padding: "4px 8px"
  table-header:
    textColor: "{colors.ink-faint}"
    typography: "{typography.label}"
    padding: "0 12px"
    height: "32px"
  table-cell:
    typography: "{typography.body}"
    padding: "0 12px"
    height: "40px"
---

# Design System: Eunomia

## Overview

**Creative North Star: "The Multiplayer Canvas"**

Eunomia is drawn as the shared canvas a team's agents and people write to, in the grammar of Figma and tldraw. A near-black ground (paper-white in the light theme) carries a faint 20px dot grid; on it float hairline panels with small radii, dense tabular data, and Geist type. Every memory says who wrote it through a Figma-style author tag, and the selected thing wears a green outline with four square handles. Dark is the default; `data-theme="light"` on `<html>` flips every token, and a saved light theme is applied before first paint.

Density is high and quiet. The page is a slim 232px sidebar plus a dot-grid canvas holding one page title, hairline ledgers and tables, and (on the dashboard) a floating inspector for the selected item. Lists of things are tables with hairline rows, not card grids. Colour is rationed into three separate vocabularies that never borrow from each other: one felt-green accent for what you can press or have selected, a six-colour presence palette for authorship, and muted kind and connector hues for categorising entities and sources.

Motion is small, fast and earned. Press feedback, popover entrances and a handful of authored moments (live memory arrival, token decrypt, digit roll, the theme wipe) are gated by frequency, by input device, and by reduced-motion preference. Real text is always in the DOM at its final value from the first frame.

**Key Characteristics:**
- Dot-grid canvas ground, hairline (1px, 0.5px on 2x screens) borders everywhere.
- One accent, felt green, only on pressable or selected things.
- Author tags in a separate presence palette mark who wrote what, and nothing else.
- Figma selection frame: 1px accent outline plus four 6px square handles.
- Dense tables with tabular numerals; mono only for numbers, ids, code and timestamps.
- Motion under 300ms for UI, pointer-only flourishes, nothing on keyboard or high-frequency actions.

## Colors

A near-neutral graphite (or paper) ground with a single felt-green accent and a separate, louder presence palette reserved for authorship.

### Primary
- **Felt Green** (`felt-green`, `--accent`): fills of the primary button, the selection frame and its handles, focus rings (2px, offset 2px), the field focus border, text selection, caret, the active nav icon, selected tabs (`pill[aria-selected]`), the brand mark tile. Light theme uses the deeper `felt-green-light`.
- **Felt Green Text** (`felt-green-text`, `--accent-text`): the green used as text on the ground: links, "Connect one" calls to action, the active nav icon, the update-available notice.
- **Felt Green Wash** (`felt-green-soft`, `--accent-soft`): the fill behind a selected table row and a selected tab pill, and the update notice background.
- **On Felt** (`on-accent`): ink that sits on a green fill.

### Secondary: the presence palette
- **Author 1 to 6** (`author-1` coral, `author-2` sky, `author-3` lilac, `author-4` ochre, `author-5` rose, `author-6` teal): the background of author tags, the author cursor glyph, the account avatar, and the 16% arrival wash on newly arrived rows. Assigned by `authorColor(name)`: known agents get fixed slots (Claude and Claude Code 1, Gemini 2, Cursor 3, Windsurf 4, Eunomia 5, Codex 6); any other name hashes stably into one of the six. Text on a tag is `on-author` (canvas ink in dark, white in light).

### Tertiary: categorical hues
- **Entity kinds** (`kind-person`, `kind-organisation`, `kind-location`, `kind-repository`, `kind-file`, `kind-symbol`): muted, desaturated hues for the kind dot beside an entity and the nodes of the entity graph. A custom kind an agent created has no hue of its own: it uses `ink-faint` and a plain circle node.
- **Connector and series hues** (`--connector-*`, `--series-1..4` in globals.css): per-source identity dots and chart series. Same muted register as the kinds; never as fills behind text.
- Kind, connector and series hues are defined once on `:root` and are **not** redefined for the light theme; they are mid-tone on purpose so they hold on both grounds.

### Status
- **Good** (`good`): healthy status dots and the drawn "done" check. Shares the felt-green value but carries status meaning, not action.
- **Warning** (`warning`) and **Critical** (`critical`, with `critical-soft` wash): status dots, error text, danger buttons, the hold-to-delete fill.

### Neutral
- **Canvas** (`canvas`): the page ground under the dot grid, and the outline stroke around the author cursor.
- **Surface** (`surface`): ledgers, panels, the sidebar and mobile top bar.
- **Surface Raised** (`surface-raised`): buttons, fields, pills, tooltips, table-row hover, the active nav row, code blocks.
- **Surface Hover** (`surface-hover`): button hover, pressed filter pills, the skeleton shimmer peak.
- **Ink / Ink Dim / Ink Faint** (`ink`, `ink-dim`, `ink-faint`): primary text; secondary text and descriptions; labels, placeholders, table headers, kbd glyphs, idle nav icons.
- **Border / Border Strong** (`border`, `border-strong`): hairline rules and container edges; button and kbd edges and the scrollbar thumb.
- **Grid Dot** (`grid-dot`): the 1px dots of the canvas grid. Light theme is a touch stronger (0.10 versus 0.07) so the grid still reads on white.
- **Scrim** (`scrim`): behind the command palette and the mobile drawer.

### Named Rules
**The Felt Rule.** Felt green appears only on something you can press or something that is selected (primary button, focus ring, selection frame, selected tab, link). Never on headings, decoration, or icons at rest.

**The Presence Rule.** Author colours mark who wrote something and nothing else. Never use them for actions, status or decoration, and never invent an author: if the data has none, show no tag.

**The Quiet Filter Rule.** Multi-select filters that are on (`aria-pressed="true"`) go neutral (raised fill, strong border, full ink), so a row of them never becomes a wall of green. Only single-choice selection (`aria-selected`) takes the accent.

**The Two Theme Rule.** Every colour is a variable with a dark and a light value. Never hard-code black, white or rgba shadows; use `scrim`, `--shadow-panel`, `--shadow-pop`.

## Typography

**Display Font:** Geist (with system-ui, sans-serif)
**Body Font:** Geist (with system-ui, sans-serif), stylistic sets `ss01` and `cv11` on
**Label/Mono Font:** Geist Mono (with ui-monospace, monospace), tabular numerals, `font-feature-settings: normal` (the body-level `ss01`/`cv11` sets are switched off on mono)

**Character:** One neo-grotesque family at small, dense sizes, tightened at headline weights; its mono sibling carries every number, id, path and timestamp so columns align.

### Hierarchy
- **Display** (600, 22px, 1.2, -0.025em, balanced wrap): the page title, one per page, top left of the canvas. The onboarding welcome heading uses the same style.
- **Headline** (600, 17px, -0.015em): pane and detail headings inside a page (chat thread title, vault detail).
- **Title** (600, 13px, -0.005em): section headings inside a page ("Live memory", "Connected sources").
- **Body** (400, 13px; root is 14px): table cells, nav rows, descriptions. Helper paragraphs hold to 62ch; long-form docs hold to 70ch at line-height 1.6.
- **Label** (400, 12px, 1.3, `ink-faint`): field labels, column headers, stat labels, palette group names. Sentence case, no tracking.
- **Stat** (Geist Mono, 26px, line-height 1, -0.02em): numeric stat values on the dashboard strip; non-numeric stat values use Geist 500 at the same size.
- **Mono** (Geist Mono, 10.5 to 13px): ids, tokens, URLs, commands, record counts, timestamps, the version tag. Always via `.font-mono` (or the `font-mono` Tailwind utility, which maps to `--font-mono`); that class sets `font-feature-settings: normal` and `font-variant-numeric: tabular-nums`. Do not set Geist Mono by hand without the reset, or the `ss01` alternates leak into code and ids.
- **Sizes in use** (px): 10.5 version tag, 11 kbd and record counts, 11.5 tags and error detail, 12 labels and helper text, 12.5 error lines and notices, 13 body, 13.5 palette rows and card titles, 14 root, 15 brand and palette input, 17 headline, 22 page title, 26 stat. Do not add steps between these.
- **Weights:** 400 body, 500 buttons, active nav, table headers and tags, 600 headings. Nothing heavier.
- **Helpers:** `.page-title`, `.section-title`, `.label` (alias `.eyebrow`), `.font-display`, `.font-mono`, `.tabular` (tabular numerals only, keep Geist).
- **Tag** (500, 11.5px): author tags; kbd glyphs are mono 500 at 11px.

### Named Rules
**The No Kicker Rule.** Headings carry their own weight. Never set a small label above a heading; `label` is for fields, columns and stats only.

**The Honest Mono Rule.** Mono is for data that must align or be copied exactly (numbers, ids, code, timestamps). Never for prose, labels or decoration.

## Layout

The app shell is a flex row: a sticky full-height sidebar (232px, `surface`, right hairline) and a `main` canvas with the dot grid (20px pitch) padded 24px by 16px on mobile and 32px by 40px from `md` (768px) up. Below `md` the sidebar becomes a 48px sticky top bar (menu, brand, search) and a 264px slide-in drawer over the scrim.

Pages stack vertically with 24 to 36px between sections (`gap-6` to `gap-9`) and cap their width (dashboard 1240px, vaults 72rem, settings and connectors `max-w-5xl` = 64rem). Settings content inside its panel caps at `max-w-3xl`. Two-column pages put the main ledger on the left and a fixed side column on the right from `lg` (1024px): 320px for the dashboard inspector (sticky at 32px), 380px for vault detail. Settings uses a 180px tab rail beside its panel from `md`, with rows laid out as a 180px label column beside the control. The entities page breaks out of the canvas padding to fill the viewport with its graph.

Spacing is a 4px-based rhythm with a heavy middle: 6 to 8px inside tight groups (icon and label, tag and text), 12px for table cell and button padding, 16 to 20px inside ledgers and panels, 24 to 40px between sections. More space sits above a heading than below it. The command palette opens 14vh from the top, 560px wide.

## Elevation & Depth

Hybrid and restrained. In-flow containers (ledgers, surfaces) are flat: a hairline border on `surface` against the dot-grid canvas. Only floating things get a shadow, and every shadow starts with a 1px ring so the edge reads as a hairline first. Depth otherwise comes from tonal steps (`canvas` to `surface` to `surface-raised` to `surface-hover`).

### Shadow Vocabulary
- **Panel** (`--shadow-panel`, dark: `0 0 0 1px var(--border), 0 1px 1px rgba(0,0,0,0.25), 0 12px 32px -12px rgba(0,0,0,0.6)`): floating panels: the dashboard inspector, onboarding and auth cards. The command palette is a `.panel` that overrides the shadow with Pop, because it sits over the scrim.
- **Pop** (`--shadow-pop`, dark: `0 0 0 1px var(--border-strong), 0 4px 12px -2px rgba(0,0,0,0.4), 0 24px 64px -16px rgba(0,0,0,0.7)`): tooltips, menus and popovers that sit above panels.
- Light theme swaps both to ink-tinted shadows at a quarter of the opacity (see the sidecar).

### Named Rules
**The Float Only Rule.** In-flow blocks never cast a shadow; a shadow means the thing floats over the canvas. Never nest a card inside a card.

## Surfaces

- **Canvas** (`.canvas-grid` on `<main>`): `--canvas` plus a `radial-gradient` dot at 1px, `--grid-dot`, on a 20px by 20px pitch. The sidebar and mobile bar are `--surface`, so the grid only shows in the work area. Put new screens inside the app layout and they inherit it; never repaint the ground.
- **Ledger / `.surface`** (`.ledger`): `--surface`, hairline `--border`, 10px radius, flat. The default container for tables, form groups and lists. Combine with `.hairline-rows` (a hairline between direct children) for stacked rows and settings forms.
- **Panel** (`.panel`): `--surface`, 10px radius, `--shadow-panel`. Only for things that float or are modal: inspector, palette, select list, freshly minted token card, auth and onboarding cards.
- **Frame selected** (`.frame-selected`): see Selection Frame. Works on any `position: relative` block, and on `tr` as outline only.
- **Raised footer strip:** the last row of a form ledger (save bar) uses `--surface-raised` with the card's bottom radii, see the settings form.

## Shapes

Small, consistent radii and one-device-pixel lines. Tags and kbd keys 4px (the author tag is 4px with a 1px bottom-left corner, so it points at its cursor like a Figma name tag); pills, tooltips and select options 6px; skeleton blocks 5px; buttons, fields and nav rows 7px; code blocks 8px; ledgers, panels and the inspector 10px; status dots and avatars fully round. Every border and rule is `--hair` (1px, 0.5px on 2x displays). The selection frame is the one recurring silhouette: a 1px accent outline inset by 1px with four 6px square handles at the corners, 3px outside the box. The brand mark is a 6px-radius green tile where three scattered points resolve into one off-centre node.

## Components

### Buttons
Compact and tactile, with a press you can feel.
- **Shape:** gently squared (7px radius), 30px tall, 12px side padding, 13px weight 500, 6px icon gap, hairline `border-strong` edge.
- **Default:** `surface-raised` fill, `ink` text. Hover (fine pointers only) moves to `surface-hover`.
- **Primary:** felt-green fill, no border, `on-accent` text; hover mixes 12% ink into the green (`color-mix(in oklab, var(--accent) 88%, var(--ink))`). One per view, the main action. It also sets `--bits-done: currentColor` so draw-in checks inside it use the button ink.
- **Ghost:** transparent, `ink-dim` text, hover to `ink`. Used for icon buttons in the sidebar footer, mobile bar and toolbars.
- **Danger:** transparent with `critical` text and a strong hairline; hover fills `critical-soft`. Destructive vault and token actions use the HoldButton variant (below).
- **Sizes:** small 26px tall, 9px padding, 12px text; icon-only is a 30px (or 26px small) square.
- **Classes:** `.btn` plus one of `.btn-primary`, `.btn-ghost`, `.btn-danger`, optionally `.btn-sm` and `.btn-icon`. Auth and onboarding submit buttons add `h-9 w-full` or `h-9 px-4` for a taller 36px hit target.
- **States:** press scales to 0.97 over 140ms `--ease-out`; disabled is 50% opacity with a not-allowed cursor; focus is the global 2px accent ring at 2px offset.

### Chips (pills)
- **Style:** 24px tall, 6px radius, 9px padding, 12px `ink-dim` text on `surface-raised` with a hairline border. Pressable pills share the 0.97 press.
- **Elements:** `.pill` on a `<span>` is a static chip; on a `<button>` it also gets the press (`button.pill:active`).
- **State:** filter off (`aria-pressed="false"`) drops to 70% opacity; filter on goes neutral (strong border, `surface-hover`, `ink`); single-choice selected (`aria-selected="true"`) takes the accent border and `felt-green-soft` fill.

### Author Tag (signature)
The multiplayer presence grammar.
- **Tag:** 20px tall, 6px padding, radius 4px 4px 4px 1px, 11.5px weight 500, author colour fill with `on-author` text, truncates inside its column; email authors show the part before the @, the full name sits in the title.
- **Cursor:** an optional arrow glyph in the author colour with a `canvas` outline, pinned to the tag's top-left corner. Shown on API-token tags in the dashboard header and, briefly, on freshly arrived memory rows.
- **Avatar:** the account avatar in the sidebar footer is a 24px circle in the user's author colour with their initial.
- **Arrival:** rows new since the previous 60s poll (never on first load) fade in over 200ms and hold a 16% author-colour wash that clears over 1.4s; up to three of them show the author cursor for 2.4s. Under reduced motion the wash (movement-free) stays and the fade is dropped.

### Selection Frame (signature)
- **In-flow and floating items:** 1px accent outline at -1px offset plus four 6px square accent handles 3px outside the corners. The dashboard inspector wears it to show what is selected.
- **Table rows:** outline plus `felt-green-soft` row fill, no handles (a pseudo-element would become an extra cell).
- **Motion:** when a pointer picked the frame, the handles grow from 3.6px to 6px and fade in over 120ms `--ease-out`; keyboard selection shows them at once.

### Cards / Containers
- **Ledger / Surface:** `surface`, hairline `border`, 10px radius, no shadow. Sections inside separate with hairline rows rather than gaps.
- **Panel:** `surface`, 10px radius, `--shadow-panel` (ring included). Inspector padding 16px; onboarding card 28px.
- **Stat strip:** one ledger split into two (mobile) or four (lg) hairline cells, each a label over a 26px value.

### Inputs / Fields
- **Style:** `surface-raised`, hairline border, 7px radius, 32px tall single-line, 12px padding, 13px text (mono for URLs, keys and tokens), `ink-faint` placeholder.
- **Focus:** the border turns felt green; no glow, no outline. Border and fill transition over 120ms.
- **Errors:** `critical` text naming the problem and the recovery, on a `critical-soft` wash.

### Navigation
- **Sidebar:** brand row (20px mark, "Eunomia" in display 15px, mono version), a search field that opens the palette with a ⌘K kbd, nav rows, a "Connected" label over live sources (status dot, name, mono record count), and an account footer above a hairline.
- **Nav row:** 32px tall, 7px radius, 13px `ink-dim` text with a 15px icon in `ink-faint` (stroke 1.75). Hover (fine pointers) and active both use `surface-raised` with `ink`; active adds weight 500 and turns the icon felt green. No motion beyond the 120ms colour fade.
- **Mobile:** 48px top bar; the drawer slides from the left over 260ms `--ease-drawer` with the scrim fading in 180ms, and closes on navigation.

### Command Palette
A 560px `.panel` (with `--shadow-pop`) 14vh from the top over the scrim: a 48px search row (15px input, esc kbd), grouped results (label headings, 36px rows at 13.5px, 7px radius), and a 36px hint footer of kbd keys. It never animates: it is keyboard-first and opened all day.

### Data Tables
Full width, 13px, tabular numerals. Headers 32px tall in 12px weight-500 `ink-faint`; cells 40px tall with 12px side padding; hairline row rules, none after the last row; row hover (fine pointers) fills `surface-raised`. Secondary columns hide below `sm` or `md` rather than wrapping.

### Tooltip
`surface-raised`, 6px radius, 4px by 8px padding, 12px text, `--shadow-pop`, optional compact 16px kbd (`shortcut` prop). Opens after 400ms of pointer hover, immediately on keyboard focus, and instantly with no animation for 300ms after a neighbour in the same group closed. Enters over 140ms from scale 0.96 toward its trigger. Visual only: the trigger keeps its own accessible name.

### Status, Loading, Empty, Error
- **Dot:** 6px circle in `good`, `warning` or `critical` (or a kind or connector hue for identity).
- **Skeleton:** `.skeleton`, 5px radius blocks shaped like the content (set height and width with utilities such as `h-4 w-24`), a `surface-raised` to `surface-hover` shimmer over 1.2s linear, looping. Match the real row height (table rows 40px) so nothing jumps when data lands. The loading region gets `aria-busy="true"` and an `aria-label` ("Loading memories"), or the skeleton is `aria-hidden` when a heading above already names it. In tables, render two skeleton `<tr aria-hidden>` rows with one block per cell (`TableSkeleton`). A `null` state means loading, an empty array means empty; never show the empty message while loading.
- **Empty:** one sentence naming what is missing plus the one action that fixes it. Inside a table, a single `<td colSpan>` 56px tall in `ink-dim`. Outside, a `.ledger` row with the sentence in `ink-dim` 13px and a `btn btn-sm` action at the right, or in the sidebar a 12.5px `ink-faint` sentence ending in an `accent-text` link ("Connect one"). Name the recovery, not just the absence.
- **Inline error (`ErrorLine`):** `<p role="alert">` at 12.5px, `px-3 py-2`, 7px radius, `critical` text on a `critical-soft` wash, shown directly under the control group that failed. Copy states what failed and what to do next ("Could not revoke that token. Reload and try again."), with the server message used when present (`e instanceof Error && e.message ? e.message : fallback`). Clear it (`setError(null)`) when the user retries. Currently copied per page (settings, vaults); a shared component is a good first extraction.
- **Field error:** `aria-invalid`, border set to `critical` inline, and a 12px `critical` sentence under the field naming the fix. Submit stays disabled while invalid.
- **Page load error:** the same `ErrorLine`, in place of the skeleton, with a reload instruction.
- **Banner (`FailureBanner`):** for a persistent system problem: `role="alert"`, 10px radius, `critical-soft` fill with a 1px `critical` border, a 15px alert-triangle in `critical`, a 13px weight-500 headline, a one-line mono 11.5px `ink-dim` detail that truncates, then `btn btn-sm` "Review" and ghost "Dismiss".
- **Success confirmation:** a `role="status"` 12.5px `good` line with `.fade-in`, or a button label flip (`Save settings` to `Saved`) with a drawn check.
- **Busy buttons:** `disabled` plus `aria-busy`, label swapped to the present participle ("Saving…"); long actions use a SyncMark inside the button.

### Kbd
`.kbd`: 18px square minimum, 4px radius, strong hairline on `surface`, mono 11px weight 500 in `ink-faint`. Inside a tooltip it shrinks to 16px with 11.5px text. Use for shortcut glyphs only (`⌘K`, `esc`, arrows); a copyable id is plain mono text, not a kbd.

### Select
The only select in the app (nine native selects were replaced). `components/Select.tsx`, default export.
- **Props:** `value: string`, `onChange(value)`, `options: { value, label, hint? }[]`, `id?` (pair with a `<label htmlFor>`), `aria-label?` (when there is no visible label), `placeholder?` (default "Select…"), `disabled?`, `mono?` (Geist Mono for label text, for model names, ids and URLs), `className?` (size and width of the trigger; default `h-8 text-[13px]`, add `w-full` in forms). `hint` renders at the right of an option in 12px `ink-faint`.
- **Trigger:** a `button` with `role="combobox"`, styled as `.field` plus `.select-trigger` (default cursor, accent border while `data-open`), a 14px chevron in `ink-faint`, selected label in `ink` or the placeholder in `ink-faint`.
- **List:** portalled to `body` as `ul.select-list.panel` (fixed, `z-index: 60`, 4px padding, `--shadow-pop`, max height 288px, at least the trigger width and 160px, flips above when there is under 160px below). Options are `.select-option` (32px, 6px radius, 13px `ink-dim`; the active row gets `surface-raised` and `ink`; the selected row gets `ink` and an accent-text check, 14px stroke 2).
- **Behaviour:** focus never leaves the trigger (`aria-activedescendant`). Arrows, Home, End, PageUp and PageDown move; Enter or Space choose; Escape closes and refocuses; Tab chooses and moves on; typing jumps by prefix (500ms window, repeated letter cycles); typing on a closed select changes the value. Outside pointerdown closes.
- **Motion:** a pointer open uses `.pop-in` from the trigger side (`transform-origin` top or bottom center); a keyboard open appears instantly. No animation on close.

### Pop-in, fade-in and drawer-in
- `.pop-in`: opacity and `scale(0.96)` to rest over 180ms `--ease-out`, via `@starting-style`. Set `transformOrigin` inline to the trigger side. For any popover, menu or freshly appearing floating card (the minted token panel uses it).
- `.fade-in`: 180ms opacity from 0. For scrims and small inline confirmations (`Saved.` line, drawer scrim).
- `.drawer-in`: `translateX(-100%)` to 0 over 260ms `--ease-drawer`. Mobile drawer only.
- Never add these to a keyboard-opened surface that opens many times (palette, selects opened by key).

### Dot, hairline rows
- `.dot`: 6px round status or identity dot; set `background` inline from a token (`var(--good)`, `var(--warning)`, `var(--critical)`, `var(--accent)`, or a kind or connector hue). Add a visually hidden text equivalent when it is the only status signal (`<span className="sr-only">`).
- `.hairline-rows`: 1px `--border` rule between direct children. Use inside a ledger instead of per-row borders.

### Markdown and docs
`.md` styles rendered markdown (chat bubbles, docs): 1.6 line height, headings 600 with -0.015em, inline code on `surface-raised` with a hairline and 4px radius, `pre` 8px radius, links in `accent-text`, tables scroll horizontally. Wrap long-form pages in `.docs` for a 70ch measure and larger headings. `.scene3d-label` is the mono 10.5px label drawn over the 3D scenes (text-shadowed with `canvas`).

### Brand mark and theme toggle
- **EunomiaMark** (`size`, default 20): the 6px-radius accent tile with three scattered nodes resolving to one; drawn in `--accent` and `--on-accent`, so it follows the theme. Used in the sidebar, mobile bar, auth shell and onboarding.
- **ThemeToggle:** a `btn btn-ghost btn-icon btn-sm` 26px wide; wrapped in a Tooltip in the sidebar footer. Persists to `localStorage["eunomia-theme"]`; the inline script in `layout.tsx` applies a saved light theme before paint. Dark removes the attribute.
- **Fonts:** `layout.tsx` loads Geist and Geist Mono from `next/font/google` as `--font-geist` and `--font-geist-mono`, and adds both variables to `<html>`. `--font-display` and `--font-sans` alias Geist; `--font-mono` aliases Geist Mono.

### Motion system and the bits library
Tokens: `--ease-out` cubic-bezier(0.23, 1, 0.32, 1) for entrances and presses; `--ease-in-out` cubic-bezier(0.77, 0, 0.175, 1) for things moving or breathing on screen; `--ease-drawer` cubic-bezier(0.32, 0.72, 0, 1) for the drawer. Durations: `--dur-hover` 120ms, `--dur-press` 140ms, `--dur-pop` 180ms, `--dur-drawer` 260ms. `--ease-in-out` is used for the chat "thinking" breath, the login canvas drift and glide, and the vault graph moves. In JS, read tokens with `cssVar("--ease-out")` at fire time so a theme swap is honoured. Hover colour and border fades use plain `ease` at `--dur-hover`; colour fades are the one place `ease` is allowed. Popovers enter with `@starting-style` from scale 0.96 and opacity 0, never from nothing.

Gating, enforced in code:
- **Frequency:** nothing animates on the palette, sidebar navigation, tab switches, typing or list keyboard navigation.
- **Pointer versus keyboard:** an `InputModeTracker` writes `data-input="pointer"|"keyboard"` on `<html>` (a lone modifier key still counts as pointer). Pointer-only flourishes (handle snap, theme icon crossfade, theme wipe, sparks, check draws, hold fill) check it or the click's `detail`; keyboard actions swap state instantly.
- **Hover:** hover styles live only inside `(hover: hover) and (pointer: fine)`.
- **Reduced motion:** a global rule strips transitions to opacity and colour and shortens animations to near zero; the bits stop their keyframes and render final states; server rendering assumes reduced motion so nothing animates before hydration.
- **Honest text:** the final text is in the DOM from the first frame; scrambles and rolls are `aria-hidden` overlays, entrances start at opacity 0.001.

The bits components (`frontend/src/components/bits/`, styles in `bits.css`):
- **SyncMark:** refresh icon at rest; a 68% arc spinning at 800ms linear while running; a check (in `good`) or cross (in `critical`) that draws over 200ms only when the status changes after mount. Used by every "Sync now", test and run button (dashboard, connectors, connector setup, chat tool rows, skill, settings, vaults, entity graph).
- **CopyButton:** the one copy control; the icon draws to a check over 180ms on pointer copy, the name reads "Copied" for 1.5s; on a primary button the check uses the button ink. Dashboard, settings, skill, docs, connector setup.
- **DigitRoll:** only changed digits roll (240ms, 30ms stagger) on a later value change, never on mount. Dashboard stat strip and record counts, connector detail.
- **DecryptReveal:** a mono scramble resolving left to right within 480ms, mount only, for single-line secrets that appear after a click. Freshly minted tokens on the dashboard and in settings.
- **HoldButton:** a danger button where click, Enter or Space confirm at once and a pointer hold of 900ms also confirms, with a `critical-soft` wash filling left to right after a 150ms delay and snapping back over 200ms. Vault deletion and settings token revocation.
- **Spark:** eight short accent lines thrown 18px from the element edge over 280ms, from success handlers started by a pointer only. Vault invite and clone success, onboarding completion.
- **BlurWords:** words settle from a 4px blur over 420ms with a 45ms stagger, once. Only the onboarding "Welcome to Eunomia" heading; it is a first-run moment, so it runs longer than UI motion.
- **Tooltip / TooltipGroup:** see Tooltip above. Sidebar footer, icon-only toolbar buttons across chat, connectors, vaults, settings, docs, skill, entity graph and 3D scenes.
- **Theme toggle:** Sun and Moon share a cell and blur-crossfade over 200ms; a pointer click grows the new theme as a circle from the button over 280ms via View Transitions; keyboard, reduced motion or no support swap instantly.

**The Licence Rule.** Files in `bits/` are original implementations inspired by React Bits (reactbits.dev). React Bits is MIT plus Commons Clause and Eunomia ships under its own license, so no React Bits source is copied; each file opens with an "Inspired by React Bits <Name> (reactbits.dev). Original implementation for Eunomia." header, colours come only from globals.css tokens, and no new dependency is added.

**The One Moment Rule.** At most one authored motion moment per surface; everything else is press feedback and pop-in entrances under 300ms.

## Accessibility rules in use

- **Focus:** `:focus-visible` draws a 2px accent outline at 2px offset on everything. Fields are the exception: they drop the outline and turn the border accent on focus or `:focus-within`. Never remove focus styling elsewhere.
- **Names:** every icon-only button has `aria-label`; tooltips are visual only and never replace it. Fields have a `<label htmlFor>` or an `aria-label`. The Select takes `aria-label` when unlabelled.
- **Live regions:** errors use `role="alert"`; saved and copied states use `role="status"` or `aria-live="polite"`. CopyButton with text announces "Copied".
- **Tabs:** settings uses `role="tablist"`/`tab`/`tabpanel` with roving `tabIndex`, arrow-key movement and `aria-controls`. Nav links set `aria-current="page"`.
- **Toggle state:** filters use `aria-pressed`; single-choice tabs use `aria-selected`. Style hooks follow the ARIA state, not a class.
- **Tables:** real `<table>` markup; an actions column header carries `<span className="sr-only">Actions</span>`; status dots have `sr-only` text.
- **Contrast:** `ink-faint` is tuned to stay legible on both grounds (`#80858e` dark, `#676c75` light) but is still a secondary tone; do not use it for text that must be read to act. Light-theme accent text is the deeper `#23744b`.
- **Touch and pointer:** hover styles only on fine pointers; the press scale works for touch. Controls are at least 26px tall, primary targets 30 to 36px.
- **Motion:** see the motion rules. Reduced motion keeps opacity and colour transitions only, drops keyframes, and renders final states. Keyboard-driven actions never animate.
- **Honest text:** the real value is always in the DOM; scrambles and rolls are `aria-hidden` overlays.
- **Language and scheme:** `<html lang="en">`, `color-scheme` set per theme so native scrollbars and form controls follow it. The native datalist arrow is hidden because it clashes with themed fields.
- **Browser chrome:** selection is accent on `on-accent`, the caret is accent, scrollbars are thin and `border-strong`, links use a 3px underline offset at 1px thickness.

## Recipes

Copy these as starting points. They use only existing classes and components. Colour always comes from tokens.

### Error state with a copyable trace id
Shows what failed, what to do, and a trace id the user can paste to support. The id is mono text (data that must be copied exactly) beside the existing CopyButton.

```tsx
import CopyButton from "@/components/bits/CopyButton";

function ErrorWithTrace({ message, traceId, onRetry }: { message: string; traceId?: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-x-3 gap-y-2 px-3 py-2 rounded-[7px]" style={{ background: "var(--critical-soft)" }}>
      <p className="flex-1 min-w-[200px] text-[12.5px]" style={{ color: "var(--critical)" }}>
        {message}
      </p>
      {traceId && (
        <span className="flex items-center gap-1">
          <span className="label">Trace</span>
          <code className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>{traceId}</code>
          <CopyButton value={traceId} label="Copy trace id" />
        </span>
      )}
      {onRetry && (
        <button onClick={onRetry} className="btn btn-sm">
          Try again
        </button>
      )}
    </div>
  );
}
```

For a toast, keep the same anatomy inside a `.panel pop-in` (bottom right, `--shadow-pop`) and do not auto-dismiss while the trace id is showing.

### Settings form row with Select
A ledger of rows: a 180px label column beside the control from `md`, a raised save bar at the foot.

```tsx
import Select from "@/components/Select";

const row = "grid gap-1.5 md:grid-cols-[180px_minmax(0,1fr)] md:gap-6 px-5 py-4 items-start";

<form className="flex flex-col gap-3" onSubmit={(e) => { e.preventDefault(); save(); }}>
  <div className="ledger hairline-rows">
    <div className={row}>
      <label htmlFor="role" className="text-[13px] font-medium md:pt-1.5">Default role</label>
      <div className="flex flex-col gap-1">
        <Select
          id="role"
          className="h-8 text-[13px] w-full"
          value={role}
          onChange={setRole}
          options={[
            { value: "member", label: "Member", hint: "Read and write" },
            { value: "viewer", label: "Viewer", hint: "Read only" },
          ]}
        />
        <span className="label">Applied to people you invite.</span>
      </div>
    </div>
    <div className="px-5 py-3 flex items-center justify-end gap-3" style={{ background: "var(--surface-raised)", borderRadius: "0 0 var(--radius-card) var(--radius-card)" }}>
      <span className="label mr-auto" aria-live="polite">{dirty ? "Unsaved changes" : ""}</span>
      <button type="submit" disabled={saving} className="btn btn-primary">{saving ? "Saving…" : "Save settings"}</button>
    </div>
  </div>
  {error && <ErrorLine>{error}</ErrorLine>}
</form>
```

### Data table with skeleton loading
`rows === null` is loading, `[]` is empty. Skeleton cells match the 40px row height.

```tsx
<div className="ledger overflow-x-auto">
  <table className="data-table">
    <thead>
      <tr>
        <th>Name</th>
        <th className="hidden sm:table-cell">Created</th>
        <th className="w-[110px]"><span className="sr-only">Actions</span></th>
      </tr>
    </thead>
    <tbody aria-busy={rows === null}>
      {rows === null &&
        [0, 1].map((i) => (
          <tr key={i} aria-hidden>
            <td><span className="skeleton block h-4" style={{ width: "60%" }} /></td>
            <td className="hidden sm:table-cell"><span className="skeleton block h-4" style={{ width: "50%" }} /></td>
            <td />
          </tr>
        ))}
      {rows?.length === 0 && (
        <tr>
          <td colSpan={3} style={{ color: "var(--ink-dim)", height: 56 }}>
            Nothing here yet. Create one above to get started.
          </td>
        </tr>
      )}
      {rows?.map((r) => (
        <tr key={r.id}>
          <td className="font-medium">{r.name}</td>
          <td className="hidden sm:table-cell font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>{relativeTime(r.created_at)}</td>
          <td className="text-right"><button className="btn btn-ghost btn-sm">Edit</button></td>
        </tr>
      ))}
    </tbody>
  </table>
</div>
```

## Do's and Don'ts

### Do:
- **Do** build from the tokens and classes in `globals.css` (`.ledger`, `.panel`, `.btn`, `.field`, `.pill`, `.data-table`, `.label`, `.page-title`, `.section-title`, `.frame-selected`); add a token rather than a raw hex.
- **Do** show authorship with the author tag and a real author (owner email, source key, or an agent name the API returns).
- **Do** mark the selected item with the selection frame (outline plus four 6px handles), or the outline plus green wash on a table row.
- **Do** put lists of things in dense tables with hairline rows, tabular numerals and columns that hide rather than wrap on small screens.
- **Do** give every pressable thing the 0.97 press over 140ms `--ease-out`, and every floating thing the pop-in from scale 0.96.
- **Do** check every screen in both themes; dark is the default.
- **Do** keep real text in the DOM at its final value from the first frame, and render final states under reduced motion.
- **Do** reuse `Select`, `CopyButton`, `Tooltip`, `AuthorTag` and the `bits` components before writing a new control; add a prop, not a sibling.
- **Do** use lucide-react icons at 14 to 16px with stroke 1.75.

### Don't:
- **Don't** put felt green on headings, decoration or idle icons; it means "press me" or "selected".
- **Don't** use author colours for actions, status or decoration, or invent an author.
- **Don't** set a small label or kicker above a heading.
- **Don't** give in-flow blocks a shadow or nest a card inside a card.
- **Don't** animate keyboard-driven or high-frequency actions (palette, nav, tabs, typing), use `transition: all`, `ease-in`, or scale from 0.
- **Don't** add hover styles outside `(hover: hover) and (pointer: fine)`.
- **Don't** use mono for prose or labels, gradient text, or emoji and unicode glyphs as icons.
- **Don't** use a native `<select>`, `title` as the only tooltip, or a new radius, colour, font or easing; the tokens above are the whole vocabulary.
- **Don't** copy React Bits source into `bits/`.
