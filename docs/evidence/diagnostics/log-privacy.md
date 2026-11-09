# Diagnostics log privacy review

Reviewed the backend's tracing call sites when adding the collector. The risk is
free text in errors and reports, even when no call intentionally logs a memory.
Examples include provider failures in `entities/extract.rs`, `cache/reflect.rs`,
`embeddings/service.rs`, `sources/scheduler.rs` and `error.rs`; reports in
`jobs/handlers.rs` and `routers/sources.rs`; and record IDs, labels and paths
elsewhere. A provider error can quote a submitted document or prompt.

The new tracing layer handles this at the output boundary. It never visits
formatted/debug messages, string fields or span values. Only code metadata and
an explicit list of numeric operational fields reach stdout or the ring. A
canary test covers formatted messages, error fields, memory, document and chat
text, including a parent span. Known secret patterns and emails are additionally
scrubbed before storage. Dependency events are excluded from this layer.

Container logs may come from old releases or third-party services. Their arbitrary
text cannot be proved content-free by regex. The updater projects only severity,
valid timestamps and approved numeric measurements. Other text becomes an
omission notice. The backend applies its scrubber again when importing the files.

This deliberately reduces log detail, including current Docker stdout. Support
can use the code location, service, severity and measurements without receiving
customer prose. OpenTelemetry export is separately configured and is outside the
bundle. The CLI fallback is raw output and needs manual review before sharing.

The script regression test also runs the actual `v2.2.0` updater from the Git tag
against a temporary checkout. It confirms that release leaves the diagnostics
marker alone. The backend timeout test checks that a stale completion ID is
ignored and a useful note is returned instead of failing the archive.

The privileged collector reads the request with `O_NOFOLLOW` and `O_NONBLOCK`,
accepts only regular files, and publishes through a directory descriptor. Tests
cover marker symlinks and FIFOs as well as secret and content canaries.
