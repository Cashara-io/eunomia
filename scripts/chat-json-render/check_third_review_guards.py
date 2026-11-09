#!/usr/bin/env python3
"""Disable each third-review fix, require an assertion failure, and restore it."""
import argparse
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('suite', choices=['backend', 'frontend'])
parser.add_argument('--logs', type=Path, default=Path('/private/tmp/chat-json-render-v4/mutations'))
args = parser.parse_args()
args.logs.mkdir(parents=True, exist_ok=True)
UI = 'frontend/src/components/chat/ui/'
BACK = 'backend/src/'
backend = [
    ('digits-in-prose', BACK+'views/ui.rs', [('if !allowed.iter().any(', 'if false && !allowed.iter().any(')], ['--lib', 'model_digits_match_resolved_values'], 'model_digits_match_resolved_values'),
    ('metric-unit-parity', BACK+'store/ontology.rs', [('SELECT entity, property, value_num, value_text, value_bool, value_date, as_of, created_at FROM entity_property', 'SELECT entity, property, value_num, value_text, value_bool, value_date, unit, as_of, created_at FROM entity_property')], ['--test', 'chat_ui', 'metric_stored_unit_override_matches_main_fixture'], 'metric_stored_unit_override_matches_main_fixture'),
    ('server-column-resolution', BACK+'views/ui.rs', [('else if numerics.len() == 1 { numerics[0] }', 'else if !numerics.is_empty() { numerics[0] }')], ['--lib', 'omitted_y_resolves_only_unambiguous_metadata'], 'omitted_y_resolves_only_unambiguous_metadata'),
]
frontend = [
    ('renderer-column-resolution', UI+'Charts.tsx', [('if (!p.x || !p.y || !p.series) return <Notice />;\n  const y = p.y;\n  const x = p.x;', 'const y = p.y || Object.keys(rows[0] || {}).find(k => rows.some(r => number(r[k]) != null)) || "value";\n  const x = p.x || "name";')], [], 'omitted y never substitutes'),
    ('proportional-range', UI+'data.ts', [(' && (n === 0 || Math.abs(n) >= 1e-100)', ''), ('return total > 0 && drawable(total) && Number.isFinite(1 / total) ? total : null;', 'return total;')], [], 'proportional charts reject subnormal and zero totals'),
]
for name, filename, replacements, tests, expected in backend if args.suite == 'backend' else frontend:
    if args.suite == 'backend':
        deadline = time.monotonic() + 3600
        while True:
            subprocess.run(['df', '-h', '/'], check=True)
            other = subprocess.run(['pgrep', '-x', 'cargo'], capture_output=True, text=True)
            print('pgrep -x cargo: ' + (other.stdout.strip() or 'none'), flush=True)
            if other.returncode != 0 and shutil.disk_usage('/').free >= 7*1024**3: break
            if time.monotonic() >= deadline: raise SystemExit('Cargo or disk gate blocked after 60 minutes')
            print('Waiting three minutes for cargo and 7 GiB free', flush=True)
            time.sleep(180)
    path = ROOT / filename
    original = path.read_text()
    try:
        mutated = original
        for old, new in replacements:
            assert old in mutated, (name, 'missing anchor')
            mutated = mutated.replace(old,new,1)
        path.write_text(mutated)
        if shutil.disk_usage('/').free < 6*1024**3: raise RuntimeError('STOP: disk below 6 GiB')
        env = dict(os.environ,CARGO_BUILD_JOBS='2',CARGO_PROFILE_DEV_DEBUG='0',CARGO_INCREMENTAL='0',RUST_MIN_STACK='8388608')
        command = ['nice','-n','19','cargo','test',*tests] if args.suite == 'backend' else ['bun','test','scripts/chat-ui-values.test.jsx']
        log = args.logs / (name+'.log')
        with log.open('w') as out:
            child = subprocess.Popen(command,cwd=ROOT/args.suite,env=env,stdout=out,stderr=subprocess.STDOUT,start_new_session=True)
            while child.poll() is None:
                if shutil.disk_usage('/').free < 6*1024**3:
                    os.killpg(child.pid,signal.SIGTERM);child.wait();raise RuntimeError('STOP: disk below 6 GiB')
                time.sleep(1)
        output = log.read_text()
        failed = '(fail) '+expected if args.suite == 'frontend' else expected+' ... FAILED'
        assert child.returncode != 0 and failed in output and 'could not compile' not in output, (name,'expected assertion failure absent',output[-2500:])
        print(name+': regression failed as expected',flush=True)
    finally:
        path.write_text(original)
print('All mutated files restored.')
