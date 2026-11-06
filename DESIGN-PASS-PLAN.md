# Design pass plan

> **Superseded.** This was the plan for the earlier "Multiplayer Canvas" pass. The app now follows the Eunomia Bauhaus app design (release 2.0.8); see [DESIGN.md](DESIGN.md). Kept for history only.

Branch: `design-pass`. Goal: make the Eunomia frontend look and feel like a top San Francisco startup product (think Linear, Vercel, Raycast, Anthropic). Calm, sharp, fast, expensive-looking. Same features, same copy facts, same routes.

Tools: `/impeccable` owns direction, tokens and review. `/emil-design-eng` owns motion and the small touches. `/pstack:swarm` fans the page work out in parallel.

## The one rule that makes this work

A swarm cannot invent a design system. Seven workers each picking their own look gives you seven apps.

So the work runs in this order:

1. **Foundation (one agent, serial).** Pick the look, write the tokens, rebuild the shared shell.
2. **Swarm (seven agents, parallel).** Each restyles its own pages using only the foundation.
3. **Finish (one agent, serial).** Review everything as one product, fix seams, add motion polish, document.

## Phase 1: Foundation (serial, me)

1. `/impeccable init`: write `PRODUCT.md` (what Eunomia is: a memory layer for AI agents; who uses it: developers running Claude Code, Codex, Cursor and friends; tone: confident, technical, quiet).
2. `/impeccable` new-work for the visual world. Starting brief:
   - Light and dark themes (today is dark only). Dark stays the default.
   - One sharp grotesk for UI (candidates: Geist, Inter Display, Söhne-like free alternatives) plus a mono for data. Tight tracking on headings, tabular numbers.
   - Neutral greys with a faint warm or cool tint, one confident accent. Replace "felt green" unless the direction keeps it.
   - Hairline 1px borders, small radii (6 to 10px), subtle layered shadows, no heavy cards.
   - Dense but breathable layout. Keyboard first (Cmd-K is already there, make it the hero).
3. Rewrite `frontend/src/app/globals.css` tokens and base classes (`.surface`, `.field`, `.pill`, `.eyebrow`, `.md`, focus ring).
4. Update `frontend/src/app/layout.tsx` fonts and theme switch.
5. Rebuild the shell: `Sidebar.tsx`, `CommandPalette.tsx`, `EunomiaMark.tsx`, `AuthGuard.tsx`, `(app)/layout.tsx`.
6. Write the motion contract from `/emil-design-eng` into `DESIGN.md`:
   - Custom ease-out curves only, nothing linear or default `ease`.
   - UI motion under 250ms. Enter from `scale(0.96)` plus fade, never from `scale(0)`.
   - Buttons press to `scale(0.97)`.
   - No animation on actions people repeat fast (typing, keyboard nav).
   - Respect `prefers-reduced-motion`.
7. Verify: `bun run build`, `bun run lint`, screenshot desktop and mobile. Commit as the base for the swarm.

## Phase 2: Swarm (parallel, 7 workers)

Shape: partition. One worker per slice. Each slice owns a set of files no other worker touches.

Model: `claude-opus-5-5` (from `~/.claude/pstack-models.md`). Each worker gets its own git worktree cut from the Phase 1 commit, and its own dev server port.

| # | Slice | Mode | Files it owns |
|---|---|---|---|
| 1 | Dashboard | Operate | `(app)/page.tsx`, `StatRing.tsx`, `SyncStatusCard.tsx`, `VectorCloud.tsx` |
| 2 | Chat | Operate | `(app)/chat/page.tsx`, `Markdown.tsx` |
| 3 | Memory graph | Experience | `(app)/entities/page.tsx`, `(app)/code/page.tsx`, `EntityGraph.tsx`, `Scene3D.tsx` |
| 4 | Connectors | Operate | `(app)/connectors/**`, `lib/connectorMeta.tsx` |
| 5 | Vaults and settings | Operate | `(app)/vaults/page.tsx`, `(app)/settings/page.tsx` |
| 6 | Auth and onboarding | Persuade | `login/page.tsx`, `register/page.tsx`, `onboarding/page.tsx` |
| 7 | Docs and skill | Read | `(app)/docs/page.tsx`, `(app)/skill/page.tsx` |

Every worker brief says:

- Read `PRODUCT.md` and `DESIGN.md` first. Use only their tokens and shell. Do not edit `globals.css`, `layout.tsx` or the shell. If you need a new token, ask for it in your report.
- Use `/impeccable` for the page (its mode is in the table) and `/emil-design-eng` for every interaction on it.
- Keep every feature, route, label meaning and `data-testid`. Playwright specs must still pass.
- Verify: `bun run build`, `bun run lint`, the Playwright specs for your pages, screenshots at desktop and phone width.
- Report `PASS`, `ISSUES` or `BLOCKED`, with screenshots and the list of token requests.

## Phase 3: Finish (serial, me)

1. Merge the seven worktrees into `design-pass` (no file overlap, so merges should be clean).
2. Add any token requests to `globals.css` in one place.
3. `impeccable-finish-reviewer`: review the whole app against the direction. Fix what it finds.
4. Emil pass on the whole app: hover and press states, popover origins, toasts, empty states, loading skeletons.
5. `impeccable-documenter`: write the final `DESIGN.md` from what shipped.
6. Full check: `bun run build`, `bun run lint`, `bun run test`, walk every page in the browser on desktop and phone, light and dark.

## Done means

- Every page uses the same tokens, type and motion. No leftover old colours.
- Build, lint and all Playwright specs pass.
- Screenshots of every page, both themes, desktop and phone.
- `PRODUCT.md` and `DESIGN.md` committed.

## Out of scope unless you say so

- A public marketing landing page (there is none today; the install page lives on GitHub Pages).
- Backend changes.
- Renaming or removing features.
