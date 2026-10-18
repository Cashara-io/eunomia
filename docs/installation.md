# Installation

Runs on Linux and macOS (Windows: inside WSL), on both **amd64** (x86-64) and
**arm64** (Apple Silicon, Raspberry Pi 4/5, Ampere/Graviton). The images are
multi-arch, so Docker pulls the native one.

## The installer

```bash
curl -fsSL https://www.buildpaperless.com.au/eunomia/install.sh | bash
# unattended, with options:
curl -fsSL https://www.buildpaperless.com.au/eunomia/install.sh | bash -s -- --yes --dir ~/eunomia
```

It runs four steps: check the machine (installing anything missing), choose a
setup, install, and connect.
Re-running it over an existing install is safe. It updates the checkout,
leaves `.env` secrets alone, and reconnects agents. An install still on
SurrealDB 2.x has its data moved to SurrealDB 3 first, as **Update now** does
(see [upgrading-to-surrealdb-3.md](upgrading-to-surrealdb-3.md)).

| Flag | Default | |
|---|---|---|
| `--dir <path>` | `./eunomia` | install location |
| `--ref <ref>` | latest release | tag, branch or `latest` |
| `--frontend-port <port>` | `3000` | web app |
| `--backend-port <port>` | `8001` | API + MCP server (bound to `127.0.0.1` only; see [deployment](deployment.md#1-https)) |
| `--domain <name>` | none (HTTPS off) | Serve at `https://<name>` with a free Let's Encrypt certificate; needs DNS pointing here and ports 80/443 open, see [HTTPS](deployment.md#1-https) |
| `--acme-email <email>` | | Email for Let's Encrypt, required with `--domain` |
| `--openai-base-url <url>` | `https://api.openai.com/v1` | any OpenAI-compatible endpoint |
| `--api-key <key>` | none | optional, see [agents](agents.md#do-i-need-an-openai-key) |
| `--email <email>` | your git email | Eunomia account to create |
| `--password <pw>` | generated | saved to `.eunomia-credentials` (0600) |
| `--agents <list>` | `auto` | `auto`, `all`, `none`, or `claude,codex,hermes,gemini,cursor,windsurf,opencode` |
| `--agent <name>` | detected | which agent is running the installer |
| `--no-agents` | | don't create an account or touch agent configs |
| `--no-install-deps` | | don't install missing git/Docker/jq, just stop |
| `--yes` | | accept every default |

`EUNOMIA_EMAIL`, `EUNOMIA_PASSWORD` and `EUNOMIA_AGENT` work too, and keep
the password out of `ps`.

Missing **git, curl, openssl, Docker or jq** are installed for you: Homebrew
on macOS (Docker comes from Colima, which is headless and needs no GUI), and
apt/dnf/yum/pacman/zypper/apk on Linux (Docker from get.docker.com). It asks
first when run interactively. Unattended, it never waits on a sudo password:
it stops and says what to install instead.

If a port is taken the installer stops before changing anything.

With `--domain` it waits for `https://<domain>` to answer and connects your
agents there. If DNS or the firewall isn't ready yet it warns and carries on
with the local address; Caddy keeps retrying the certificate, and Settings,
HTTPS shows when it's active.

## Installing as an AI agent

If you're an agent installing Eunomia for your operator, run:

```bash
curl -fsSL https://www.buildpaperless.com.au/eunomia/install.sh | bash -s -- --yes
```

With no terminal it runs unattended. It detects which agent you are (Claude
Code, Codex, Gemini CLI, Cursor, OpenCode, Hermes) and adds the Eunomia MCP
server to **your** config with its own API token. Pass `--agent <name>` if
you aren't detected. Then:

1. Tell the operator the URL and the sign-in printed at the end.
2. Ask the operator to restart you, or start a new session yourself, so the new MCP server loads. Then call `tools/list`.
   You should see `recall`, `memory_write`, `docs`, and the rest.
3. Keep it private: localhost, a LAN, or a tailnet. Never expose it publicly
   without TLS ([deployment](deployment.md)).

## Manual install

```bash
git clone https://github.com/Cashara-io/eunomia.git && cd Eunomia
cp .env.example .env   # set JWT_SECRET, ENCRYPTION_KEY, SURREAL_PASS: openssl rand -base64 32
docker compose pull && docker compose up -d
./scripts/connect-agents.sh --email you@example.com --password '…'   # optional
```

## Verify

```bash
curl -fsS http://localhost:8001/healthz
docker compose ps     # from the install dir: three services up
```

## Uninstall

```bash
cd eunomia && docker compose down -v   # -v also deletes the database
rm -rf ~/.config/eunomia ~/.claude/skills/eunomia-memory   # hook + skill files, if connected
```
