#!/usr/bin/env python3
"""Run manually: prove admin and scrub tests fail when their guards are removed.
Each cargo invocation obeys the shared-Mac resource rules. Exit 75 means retry later.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
env = {**os.environ, "CARGO_BUILD_JOBS": "2", "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_INCREMENTAL": "0", "RUST_MIN_STACK": "8388608"}
cases = [
    ("admin", "backend/src/diagnostics/mod.rs", '    crate::routers::update::require_admin(&state, &user, "download diagnostics. Ask your instance admin for help").await?;', '', ["--test", "diagnostics", "admin_zip_is_content_free_and_member_is_forbidden"]),
    ("scrubbing", "backend/src/diagnostics/privacy.rs", '    let mut out = crate::redact::redact(text, &mut Vec::new());', '    return text.to_string();\n    #[allow(unreachable_code)]\n    let mut out = crate::redact::redact(text, &mut Vec::new());', ["--lib", "diagnostics::privacy::tests::removes_emails_credentials_jwts_and_private_keys"]),
]
selected = set(sys.argv[1:])
assert selected.issubset({"admin", "scrubbing"}), "guards: admin, scrubbing"
for name, relative, old, new, tests in cases:
    if selected and name not in selected:
        continue
    subprocess.run(["df", "-h", "/"], check=True)
    busy = subprocess.run(["pgrep", "-x", "cargo"], stdout=subprocess.DEVNULL).returncode == 0
    if busy or shutil.disk_usage('/').free < 7 * 1024**3:
        print("Resource gate: wait three minutes before retrying.")
        sys.exit(75)
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
