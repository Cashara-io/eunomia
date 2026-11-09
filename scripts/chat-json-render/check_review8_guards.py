#!/usr/bin/env python3
"""Restore the identifier rule for selectors and require the digit-column regressions."""
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
LOGS = Path(os.environ.get('REVIEW8_LOGS', '/private/tmp/eunomia-pr203-r8-fix'))
LOGS.mkdir(parents=True, exist_ok=True)
deadline = time.monotonic() + 7200
while True:
    subprocess.run(['df', '-h', '/'], check=True)
    other = subprocess.run(['pgrep', '-x', 'cargo'], capture_output=True, text=True)
    print('pgrep -x cargo: ' + (other.stdout.strip() or 'none'), flush=True)
    if other.returncode == 1 and shutil.disk_usage('/').free >= 7 * 1024**3:
        break
    if time.monotonic() >= deadline:
        raise SystemExit('Cargo/disk gate blocked for two hours')
    time.sleep(180)

path = ROOT / 'backend/src/views/ui.rs'
original = path.read_bytes()
anchor = '    for key in selectors(props) {\n        let known ='
try:
    source = original.decode()
    assert source.count(anchor) == 1, 'mutation anchor must be unique'
    path.write_text(source.replace(anchor, '    for key in selectors(props) {\n        prose::identifier(key)?;\n        let known =', 1))
    env = dict(os.environ, CARGO_BUILD_JOBS='2', CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', RUST_MIN_STACK='8388608', RUST_TEST_THREADS='1', TEST_HARDENED='1')
    log = LOGS / 'selector-identifier-mutation.log'
    with log.open('w') as output:
        code = subprocess.call(['nice', '-n', '19', 'cargo', 'test', '--lib', 'selectors_trust_'], cwd=ROOT / 'backend', env=env, stdout=output, stderr=subprocess.STDOUT)
    output = log.read_text()
    assert code != 0 and 'could not compile' not in output, output[-3000:]
    for name in ['explicit_digit_columns', 'inferred_digit_columns', 'table_digit_columns_and_server_labels', 'group_and_aggregate_digit_columns']:
        assert 'selectors_trust_' + name + ' ... FAILED' in output, output[-3000:]
    print('Identifier-rule mutation: all four digit-column tests failed by assertion.', flush=True)
finally:
    path.write_bytes(original)
    assert path.read_bytes() == original
    print('Restored exact source bytes.', flush=True)
