---
name: Eunomia
description: Shared memory for the AI apps people already use. Eunomia Bauhaus app design, spec v1.0 (05 Nov 2026).
colors:
  bauhaus-app-bg: "#FAFAF7"
  bauhaus-app-ink: "#171717"
  bauhaus-app-muted: "#61615D"
  bauhaus-app-line: "#DADAD2"
  bauhaus-app-soft: "#EDEDE6"
  bauhaus-app-red: "#CF412F"
  bauhaus-app-blue: "#3159B6"
  bauhaus-app-yellow: "#E8BE39"
  bauhaus-app-white: "#FFFFFF"
  bauhaus-app-red-text: "#B2382A"
  bauhaus-app-blue-hover: "#284A98"
typography:
  family: "Geist (next/font/google), Geist Mono for code"
  page-title: "44px / 600 / 1.1 / -1.4px"
  connection-heading: "34px / 600 / -1px"
  section-title: "17px / 600"
  intro: "16-17px"
  body: "13-16px"
  button: "14-15px / 600"
  secondary-label: "11-13px"
icons: "lucide-react: navigation 20px, section/page 30-32px, actions 18px"
---

# Eunomia design system

Source of truth for the web app. It implements the **Eunomia Bauhaus app
design specification v1.0** (current canvas edition, 05 Nov 2026): blue
primary actions, a dark ink sidebar, Lucide icons and 4-5px corners. Where the
spec PDF notes that the repository brief in PR #130 proposed a different
direction (ink pill buttons, light navigation, Phosphor icons), this file
follows the PDF. Do not mix the two.

Shipped in release 2.0.8. The previous "Multiplayer Canvas" system (dark
default, felt-green accent, dot grid) is retired; `DESIGN-PASS-PLAN.md` is kept
only as history.

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
6. **No decoration behind working content.** No photos, gradients or dot grids
   on task screens. Not every paragraph is a card.

## Language

Visible labels use these names. Routes and API names stay as they were.

| Visible label | Route | API / tools |
|---|---|---|
| Home | `/` (+ `/connect/<app>`, `/connect/help`) | — |
| Chat | `/chat` | — |
| People & topics | `/entities` | entities |
| Memory spaces | `/vaults` | vaults |
| Documents | `/documents` | documents |
| Connected sources | `/connectors` | connectors, sources |
| Help | `/docs` | docs |
| Settings | `/settings?tab=…` | — |
| AI instructions *(Advanced)* | `/skill` | memory skill |
| Code map *(Advanced)* | `/code` | code entities |

"Entities", "Vaults", "Connectors", "Docs", "Memory skill" and "Dashboard" are
not user-facing words any more. In the README and docs, the technical term may
follow in brackets ("memory spaces (vaults)").

## Colour

Tokens live in `frontend/src/app/globals.css` with the `--bauhaus-app-` prefix
(exact canvas values above). The older semantic tokens every component uses
(`--canvas`, `--surface`, `--ink`, `--ink-dim`, `--accent`, `--critical`, …)
are mapped onto them, so one palette drives the whole app. Any older `eu-*`
landing-page tokens are left alone.

| Role | Token | Rule |
|---|---|---|
| Page ground | `bg` #FAFAF7 | Every page. No dot grid. |
| Text, navigation, rules | `ink` #171717 | Body text; sidebar background; 2px header rule; dialog and task-panel edges; table header rows. |
| Secondary text | `muted` #61615D | Introductions, hints. Passes AA on white and bg. |
| Dividers | `line` #DADAD2 | Between rows and steps. Field edges use a darker #8F8F87 so inputs are visible. |
| Soft surface | `soft` #EDEDE6 | Copy-details blocks, help asides, hover. |
| Action / selected | `blue` #3159B6 | Primary buttons, selected nav item, selected tab, links, focus ring. Text on blue is cream/white. |
| Danger / failing | `red` #CF412F (text #B2382A) | Destructive confirms and failure status only. |
| Highlight | `yellow` #E8BE39 | Fills and shapes only, with ink text on top. Focus outline on the ink sidebar. |

Contrast: target WCAG AA (4.5:1 body, 3:1 large text and UI graphics). Never
put yellow or pale grey small text on white.

The app has one light theme; the spec defines no dark variant.

## Type

Geist throughout (`next/font/google`, `--font-geist`), Geist Mono for commands
and IDs. Page title `.page-title` 44/600/1.1/-1.4px (clamps down on narrow
screens). Connection and task headings `.connect-title` 34/600/-1px. Section
headings `.section-title` 17/600. Introductions `.page-intro` 16px muted.
Upper-case group labels `.caps` 11px/600/0.06em.

## Layout

- **App shell:** 232px ink sidebar (`components/Sidebar.tsx`) + content.
  Content padding 38px vertical / 48px horizontal at desktop, less on small
  screens. Section gap 24-30px. Long content scrolls naturally; never hard-code
  the canvas heights as a viewport.
- **Sidebar:** wordmark; Search ⌘K; main nav (Home, Chat, People & topics,
  Memory spaces, Documents, Connected sources, Help, Settings); an *Advanced*
  group (AI instructions, Code map); footer with source status in words,
  version + update status, and the signed-in account with Log out. The current
  page has a blue fill and `aria-current="page"`; focus is a yellow outline, so
  it never looks like selection. Below 768px it becomes a drawer
  (`role="dialog"`, Escape closes it, focus moves in and back).
- **Page header** (`components/ui/PageHeader.tsx`): 30-32px Lucide icon or
  provider mark, the title, optional actions on the right, a 2px ink rule, then
  one introduction sentence.
- **Task panel** (`.task-panel`): a 1.5px ink frame around the one thing to do
  on a screen (the app picker, a connection guide, a confirmation flow).
- **Source catalogue:** two columns, 36px gap; cards 20px padding, 14px gaps,
  32px marks.

## Components

| Component | Where | Contract |
|---|---|---|
| Buttons | `.btn`, `.btn-primary`, `.btn-ghost`, `.btn-danger`, `.btn-danger-solid`, `.btn-sm`, `.btn-icon` | Primary: blue fill, cream text. Secondary: white with 1px ink edge. Radius 4px. Min height 40px (32px small), 44px on touch screens. Danger only for destructive actions: red edge on the trigger, solid red on the final confirm. |
| Field | `components/ui/Field.tsx`, `.field` | Persistent visible label, hint and inline error tied with `aria-describedby`; required stated in words; visible edges; blue focus edge. |
| Status | `components/ui/StatusLabel.tsx` | Word + small shape (good, warning, critical, info, neutral). |
| Confirmation | `components/ui/ConfirmDialog.tsx` | Native `<dialog>`: names the target in the title, states the consequence, clear Cancel (focused first), optional type-the-name lock for irreversible actions, focus returns to the opener. |
| Tabs / single choice | `.pill[aria-selected]`, `.pill[aria-checked]` | Selected = blue fill. Arrow keys move. |
| Tables | `.data-table` | Ink header row with cream upper-case labels; 44px rows. |
| Code to copy | `CommandBlock` in `components/connect/Guide.tsx` | Ink block, then one primary "Copy command" button. |

## Connection flow (Home)

1. **Choose the AI app.** Home (`/`) asks "Which AI app do you use?" with
   ChatGPT, Claude Code, Codex and Other apps. Nothing is preselected. Below:
   "Or start with your own information" (Upload a document, Browse people &
   topics).
2. **Follow that app's steps** (`/connect/chatgpt`, `/connect/claude-code`,
   `/connect/codex`, `/connect/other`). Browser setup (ChatGPT) and terminal
   setup (Claude Code, Codex: Mac / Windows / Linux) are kept separate.
   Commands match `docs/agents.md` (OAuth, no pasted token). An *Advanced*
   section keeps the API-token method.
3. **Sign in and approve** on Eunomia's consent page, which explains the
   access in plain words before Allow.
4. **Confirm and try it.** "Check connection" asks the server. Success (the
   "Connection approved" view) appears only when Eunomia has an approved
   sign-in for that app or a token the app has actually used. It offers an
   example question and a route back.

Connection help (`/connect/help`) is linked from every guide. Once an app is
connected, Home shows which apps are connected (in words, with last use), then
stats, live memory and connected sources.

## Sources: connect, then import

Keep connection status separate from imported-content status: choose source →
connection checked → ready to import (destination and scope shown) → importing
(honest status) → imported (count + way to inspect). Sign-in/permission
failures and import failures have different recovery actions. Destination
changes affect future imports; moving existing items is a separate, confirmed
action. Connection names are shown so two accounts of one service differ.

## Memory spaces

Explain scope before people commit. Copy creates a new space; combining two
spaces creates a new combined space and leaves both originals unchanged.
Pending invitations grant no access. Show the person's role and enforce the
last-owner rule. Delete, leave and move name the target and consequence and
offer Cancel.

## Accessibility and responsive contract

- Every action keyboard operable; visible focus distinct from selection.
- 44px touch targets on coarse pointers; test 200% zoom and 375-390px widths;
  no horizontal page overflow; cards and app choices stack.
- Dialogs manage focus and return it to the invoking control.
- Persistent labels, inline errors with correction guidance.
- `prefers-reduced-motion` turns movement off.
- Announce meaningful completion and errors (`aria-live`), not per-frame updates.
- Minimise exposed secrets: tokens are masked, a new token is shown once.

## Not part of the released product

Do not design or build these as if they exist: a hosted script editor for
extensions (extensions run on your own machine), a skill-library screen (skill
save/find/get/delete exist only as tools; the AI instructions page is a
different thing), Pocket original audio, hosted extension runtimes and
universal instance sync.

## Assets

The official ChatGPT, Claude Code, Codex and provider logo SVGs referenced by
the spec (`handoff/assets/logos`, SVGL, Simple Icons) are not bundled in this
repository. Until they are added with their licences, the app uses Lucide
category symbols beside the visible names (`components/connect/apps.tsx`,
`lib/connectorMeta.tsx`). Up Bank uses a banking symbol and PocketAI a
microphone, as the spec allows.
