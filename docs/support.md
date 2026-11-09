# Getting help with Eunomia

Open **Settings > Support > Diagnostics** and select **Download diagnostics**.
Only instance admins can download the bundle. Members can ask their instance
admin to do this. Sign in with a browser session, API tokens cannot download it.
The app currently has no step-up authentication flow.

The download can take about a minute while Eunomia waits for container logs.
Review the zip before sending it to support. Eunomia never sends it automatically.

## Inside the zip

- `summary.json` and `summary.md`: application and database versions, image tags
  when available, estimated install date from the oldest account, process uptime,
  OS, architecture, time zone, migration ledger versions by opaque organisation ID,
  feature flags, setting names with values hidden, and model endpoint host only.
  Data-volume free space is included if the updater can read it. Unknowns are null.
- `connectors.json`: source types, status, checkpoint times, error categories,
  persisted last-success counts, cursor character counts and recent sync history.
  The last 20 queued sync jobs are included while retained by the job queue.
  Process-local history keeps up to 20 runs per connection from the latest 2,000 runs in this
  backend process. It resets on restart. Counts, durations, partial results and
  rate-limited failures are recorded. Older checkpoints have no duration history.
- `backend.log`: the latest 5,000 operational events or 5 MiB, whichever fills first.
- `containers/`: compose service status, images and up to 300 recent log lines per
  service. Docker reads are bounded to 1 MiB per command and six seconds each.
- `health.json`: `/healthz`, readiness, the latest 50 job failures by code and
  embedding backlog counts, plus ready, running and dead embedding/indexing jobs. Eunomia has `/healthz` and `/readyz`, not `/health`.
- `README.txt`: collection limits, exclusions and review instructions.

Credentials, emails, organisation and connection names, cursor values, memory,
document and chat text are excluded. Settings values are redacted, except safe
boolean flags. Model endpoints contain only the host, with no credentials, path
or query. Errors are reduced to categories because a provider can quote customer
content in an error. Logs keep timestamps, severity, code locations and numeric
measurements. Formatted messages and string/span fields are deliberately omitted
from backend stdout as well as the diagnostics ring. Plain container log text is
replaced with a privacy notice. Existing OpenTelemetry export is independent of
this download and retains its current configuration.

The archive uses stored ZIP entries and streams from a private temporary file,
removed when the download ends or disconnects. The maximum is 25 MiB including ZIP
overhead. If capped, `LIMITS.txt` names omitted files and Settings shows a message.
Collection also limits organisations, connections and settings to 1,000 each across the instance; summary notes
report those limits. Generating a bundle records a `server.diagnostics` audit event.

## How container collection works

The backend has no Docker socket. It writes a random request ID into
`update-status/diagnostics-requested`. The updater checks this before its release
check throttle on each 20-second cycle. It writes only privacy-filtered files to
`update-status/diagnostics/`, then atomically publishes the matching ID in `ready`.
The backend ignores results from earlier requests and waits at most 60 seconds.
One bundle may be generated at a time per backend process.

If the updater is stopped, busy, inaccessible, or from 2.2.0 or earlier, the zip
still downloads with a note explaining why container logs are absent. Old
updaters ignore the marker. The updater script comes from the installed release,
as described in [releasing](releasing.md). Container diagnostics only become
available after updating to a release that contains this collector.

## When the web app is down

Run these commands from the Eunomia installation folder:

```sh
docker compose ps --all
docker compose logs --no-color --tail 300 backend frontend surrealdb updater
```

CLI output is raw and may contain private information. Review it and remove
personal content, email addresses and secrets before sharing it. Do not send
`.env`, database exports, uploaded documents or credentials to support.
