# Quickstart

## 1. Install

You need git and Docker (with the Compose plugin), running.

```bash
curl -fsSL https://midhunkumar05.github.io/eunomia/install.sh | bash
```

The installer asks a few questions (Enter accepts each default), shows a
summary, then:

- starts SurrealDB, the backend and the web app,
- installs anything missing (git, Docker, …),
- turns on one-click updates (Settings → Updates),
- creates your account and connects your AI agents over MCP, with the memory
  skill and (for Claude Code) automatic recall on every prompt.

## 2. Open it

Go to **http://localhost:3000** and sign in with the email and password you
chose (if you let it generate one, it's printed at the end and saved in
`eunomia/.eunomia-credentials`).

## 3. Use it from your agent

Restart your agent so it loads the new server (in Claude Code: exit, then `claude --continue`; `/mcp` alone doesn't pick up servers added mid-session), then ask it something like:

> Remember that Ada prefers async updates over meetings.

> What do you know about Ada?

The agent calls Eunomia's `memory_write` and `recall` tools. No OpenAI key is
needed: your agent is the model.

Agent not connected yet (or installed it later)? The **Connect your AI** step
when you first sign in, and the **Connect an MCP client** card on the
dashboard, give the exact setup for Claude Code, Codex and ChatGPT with your
URL and a fresh token filled in. See [AI agents & MCP](agents.md#connecting-one-agent-by-hand).

## 4. Optional: add a model key

Settings → OpenAI. Set the **base URL** first (any OpenAI-compatible
endpoint), then the key. It's optional but recommended, because it saves your
agent's tokens: Eunomia then does embeddings (semantic search) and answer
synthesis (`reflect`) itself, and the in-app Chat works.

## 5. Tune what agents remember

**Memory skill** in the sidebar shows the instructions agents follow. You
can edit them, for example "always remember my project deadlines".

## Next

- [AI agents & MCP](agents.md): connect more agents and see every tool.
- [Concepts](concepts.md): vaults, memories, the graph and the vector cloud.
