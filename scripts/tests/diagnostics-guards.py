#!/usr/bin/env python3
"""Run manually: prove diagnostics tests fail when their guards are removed.
Each cargo invocation obeys the shared-Mac resource rules. Exit 75 means retry later.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

root = Path(__file__).resolve().parents[2]
env = {**os.environ, "CARGO_BUILD_JOBS": "2", "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_INCREMENTAL": "0", "RUST_MIN_STACK": "8388608"}
cases = [
    ("admin", "backend/src/diagnostics/mod.rs", '    crate::routers::update::require_admin(&state, &user, "download diagnostics. Ask your instance admin for help").await?;', '', ["--test", "diagnostics", "admin_zip_is_content_free_and_member_is_forbidden"]),
    ("scrubbing", "backend/src/diagnostics/privacy.rs", '    let mut out = crate::redact::redact(text, &mut Vec::new());', '    return text.to_string();\n    #[allow(unreachable_code)]\n    let mut out = crate::redact::redact(text, &mut Vec::new());', ["--lib", "diagnostics::privacy::tests::removes_emails_credentials_jwts_and_private_keys"]),
]
cases.extend([
    ("numeric-allowlist", "backend/src/diagnostics/logs.rs",
     'if let Some([low, high]) = self.spec.and_then(|s| s.fields.get(field.name()))\n            && (*low..=*high).contains(&value)',
     'if true', ["--lib", "diagnostics::logs::tests::content_and_numeric_secrets_never_enter_ring_or_zip"]),
    ("regular-files", "backend/src/diagnostics/mod.rs", 'if !metadata.is_file() || metadata.len() > FILE_LIMIT',
     'if metadata.len() > FILE_LIMIT', ["--lib", "diagnostics::io_tests::unsafe_files_are_omitted_with_deadline_and_next_bundle_succeeds"]),
    ("nofollow", "backend/src/diagnostics/mod.rs", 'libc::O_NONBLOCK | libc::O_NOFOLLOW',
     'libc::O_NONBLOCK', ["--lib", "diagnostics::io_tests::unsafe_files_are_omitted_with_deadline_and_next_bundle_succeeds"]),
    ("read-deadline", "backend/src/diagnostics/mod.rs",
     'tokio::time::timeout_at(deadline, tokio::task::spawn_blocking(move || read(&path))).await.ok()?.ok()?.ok()',
     'let _ = deadline; tokio::task::spawn_blocking(move || read(&path)).await.ok()?.ok()',
     ["--lib", "diagnostics::io_tests::slow_or_panicking_reads_release_bundle_lock"]),
    ("nonblocking-logging", "backend/src/diagnostics/logs.rs", 'self.ring.try_lock() { ring.push(line); }',
     'self.ring.lock() { ring.push(line); }', ["--lib", "diagnostics::logs::tests::logging_drops_instead_of_waiting_for_snapshot_lock"]),
])
selected = set(sys.argv[1:])
assert selected.issubset({case[0] for case in cases}), "unknown diagnostics guard"
for name, relative, old, new, tests in cases:
    if selected and name not in selected:
        continue
    subprocess.run(["df", "-h", "/"], check=True)
    busy = subprocess.run(["pgrep", "-x", "cargo"]).returncode == 0
    started = time.monotonic()
    while busy or shutil.disk_usage('/').free < 7 * 1024**3:
        if time.monotonic() - started >= 7200: sys.exit(75)
        print("Resource gate: waiting three minutes.", flush=True)
        time.sleep(180)
        subprocess.run(["df", "-h", "/"], check=True)
        busy = subprocess.run(["pgrep", "-x", "cargo"]).returncode == 0
    path = root / relative
    source = path.read_text()
    assert old in source
    try:
        path.write_text(source.replace(old, new, 1))
        result = subprocess.run(["nice", "-n", "19", "cargo", "test", *tests, "--", "--nocapture"], cwd=root / "backend", env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        log = Path('/tmp') / f'eunomia-diagnostics-guard-{name}.log'
        log.write_text(result.stdout)
        if result.returncode == 0 or 'test result: FAILED' not in result.stdout:
            raise RuntimeError(f"{name}: mutation not killed by a test assertion, see {log}")
        print(f"PASS: {name} mutation killed by assertion, {log}")
    finally:
        path.write_text(source)
