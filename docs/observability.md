# Observability (optional)

Cupel ships an optional self-host observability stack: [SigNoz](https://signoz.io) for traces, logs and metrics. **It is off by default.** A normal install never starts it and the backend exports nothing unless you set `OTEL_EXPORTER_OTLP_ENDPOINT`.

## Resource needs

Four long-running containers (SigNoz UI, OTLP collector, ClickHouse, ClickHouse Keeper) plus two one-shot setup jobs. Measured idle on an empty install: about 850 MiB RAM (ClickHouse ~670 MiB, Keeper ~95 MiB, SigNoz ~50 MiB, collector ~35 MiB). Plan on 1.5 to 2 GiB of free RAM under load and a few GB of disk. The first start also downloads about 1 GB of images and needs internet once (a helper fetches SigNoz's `histogramQuantile` binary from GitHub).

## Turn it on

1. Set the endpoint in `.env`:

   ```
   OTEL_EXPORTER_OTLP_ENDPOINT=http://signoz-otel-collector:4317
   ```

   (`OTEL_SERVICE_NAME=eunomia-backend` is already set in `docker-compose.yml`. gRPC is `:4317`, HTTP is `:4318`.)

2. Start the stack with the observability file and profile:

   ```bash
   docker compose -f docker-compose.yml -f deploy/observability/docker-compose.yml \
     --profile observability up -d
   ```

3. Open the UI at <http://127.0.0.1:8080> and create the admin account. **Do this before expecting data:** the collector only starts listening on 4317/4318 once SigNoz has an organisation, which the first sign-up creates.

The UI is bound to `127.0.0.1` only. To reach it from another machine use an SSH tunnel (`ssh -L 8080:127.0.0.1:8080 host`) rather than opening the port. Change host ports with `SIGNOZ_UI_PORT`, `SIGNOZ_OTLP_GRPC_PORT`, `SIGNOZ_OTLP_HTTP_PORT` if they clash.

To turn it off: drop the `-f ... --profile observability` flags and unset the endpoint, then `docker compose up -d` again. Data lives in the `eunomia-obs-*` volumes; `docker volume rm` them to delete it.

Running the backend outside Docker (`cargo run`)? Use `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4317`.

## Find a request by trace id

Errors in the UI, API (`trace_id` in the problem+json body) and MCP tool results carry a trace id.

1. In SigNoz open **Traces** and add the filter `trace_id = <id>` (or paste the id into the Traces search box).
2. Click the trace to see every span: route, registry tool, store queries, job attempts, with timings and errors.
3. For the logs of that request open **Logs** and filter `trace_id = <id>`.

## Let agents query it (SigNoz MCP server)

SigNoz publishes an official MCP server: <https://github.com/SigNoz/signoz-mcp-server> (pinned here at `v0.15.0`).

1. In the SigNoz UI go to **Settings, API Keys** and create a key. Keep it out of git.
2. Run the server over stdio from Claude Code (needs Docker; the container reaches the host UI via `host.docker.internal`):

   ```bash
   claude mcp add signoz \
     -e SIGNOZ_URL=http://host.docker.internal:8080 -e SIGNOZ_API_KEY=<key> \
     -- docker run -i --rm -e SIGNOZ_URL -e SIGNOZ_API_KEY signoz/signoz-mcp-server:v0.15.0
   ```

   For Codex, in `~/.codex/config.toml`:

   ```toml
   [mcp_servers.signoz]
   command = "docker"
   args = ["run", "-i", "--rm", "-e", "SIGNOZ_URL", "-e", "SIGNOZ_API_KEY", "signoz/signoz-mcp-server:v0.15.0"]
   env = { SIGNOZ_URL = "http://host.docker.internal:8080", SIGNOZ_API_KEY = "<key>" }
   ```

   Alternative: a prebuilt binary from the project's releases with `SIGNOZ_URL`/`SIGNOZ_API_KEY` set, no Docker needed (then `SIGNOZ_URL=http://127.0.0.1:8080`).

3. Check with `/mcp` in the client. Then ask the agent things like "show the failing traces for service eunomia-backend in the last hour" or "fetch trace <id> and its logs".

The stdio form above is what we have not run against a live key here; the HTTP mode (`TRANSPORT_MODE=http`, `MCP_SERVER_HOST=127.0.0.1`) is the documented alternative if your client prefers a URL.

## Notes

- Compose files were generated with SigNoz's own tool, Foundry (`foundryctl` v0.3.0), because SigNoz removed the old `deploy/docker` compose from its repo. Images are pinned: `signoz/signoz:v0.145.0`, `signoz/signoz-otel-collector:v0.144.12`, ClickHouse and Keeper `25.12.5`. Bump them together, following SigNoz's upgrade notes.
- Config for the collector and ClickHouse is under `deploy/observability/`.
