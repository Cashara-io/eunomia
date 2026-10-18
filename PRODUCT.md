# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Engineering teams and companies that run AI coding and work agents (Claude Code, Codex, Cursor, Windsurf, Gemini CLI, Hermes, OpenCode, VS Code, Claude Desktop). An admin installs Eunomia, connects the team's agents over MCP, and sets up shared vaults. Engineers then live in their agents; they open the web app to check what is remembered, browse the graph, manage vaults and connectors, mint tokens, and chat with memory.

## Product Purpose

Eunomia is the shared, long-term memory layer for a team's AI agents. Every agent reads and writes the same memory through one tool registry, so no one has to re-explain people, projects, decisions or code to an agent again. Success: agents answer with the team's context on the first try, and people can see and trust what the agents know.

## Positioning

Two equal halves:

1. Memory for your agents: one memory every agent plugs into over MCP, scoped per user or per team vault.
2. See everything you know: an entity graph, a code graph and a vector cloud make the memory visible and inspectable, not a black box.

Mechanism: a single SurrealDB schema (rows, full-text, vectors, graph relations) behind one tool registry used identically by the UI, the built-in chat agent and external agents. Recall fuses five retrieval arms (semantic, keyword, memory text, graph, temporal). Observations consolidate raw facts into evolving beliefs per entity.

## Operating Context

Self-hosted (Docker Compose or `./run.sh`), Rust/Axum backend on :8001, Next.js frontend on :3000 proxying `/api`. Installed with a one-line `curl | bash` script that also wires up agents. Users sign in with a session cookie; other clients use personal API tokens. Works without an OpenAI key (the agent is the model).

## Capabilities and Constraints

- Routes: dashboard, chat, entities (graph, cloud), code graph, connectors (marketplace, per-connector detail and setup), vaults (personal and org, invite, clone, merge), settings (tokens, updates), docs, skill, login, register, onboarding.
- Terminology: vault, entity (person, organisation, location, repository, file, symbol), memory (world, experience, observation), record, source/connector, recall, reflect, consolidate.
- Connectors: Up Bank, HeyPocket, Open Connector, demo, plus a marketplace of others.
- Playwright specs in `frontend/tests` depend on existing routes, labels and test ids.
- Next.js 16 with breaking changes; read `frontend/node_modules/next/dist/docs/` before using framework APIs.

## Brand Commitments

- The name "Eunomia" is fixed.
- The logo mark may be redrawn.

## Evidence on Hand

- No customer logos, testimonials, usage numbers or benchmarks exist. Do not invent any.
- License: PolyForm Noncommercial 1.0.0 (free for personal use; commercial use is paid).

## Product Principles

1. Trust through visibility: show what agents know, where it came from and how fresh it is.
2. Agents first, humans in control: the UI is where people inspect, correct and govern memory.
3. One system, one vocabulary: the same tools and terms in UI, chat and MCP.
4. Team-scale calm: dense information that stays readable and quiet.

## Accessibility & Inclusion

WCAG 2.2 AA as the working standard. Respect reduced motion. Full keyboard operation.
